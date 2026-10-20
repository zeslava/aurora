use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

mod auth;
mod config;
mod database;
mod ffmpeg;
mod metadata;
mod metrics;
mod models;
mod pairing;
mod scanner;
mod segments;
mod server;
mod static_files;
#[cfg(test)]
mod testkit;
mod tmdb;
mod transcoder;

use config::Config;
use database::Database;
use metrics::MetricsCollector;
use pairing::PairingManager;
use server::{AppState, create_router};
use transcoder::HlsManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Загрузка конфигурации
    let config = Config::from_env()?;

    // Инициализация логирования с выводом в stderr для Docker
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level)),
        )
        .init();

    eprintln!("Запуск Aurora медиасервера...");
    info!("Запуск Aurora медиасервера...");
    info!(
        "Конфигурация загружена: порт={}, медиатеки={}, БД={}",
        config.port,
        config.media_paths.len(),
        config.database_url
    );

    // Инициализация базы данных
    info!("Подключение к базе данных...");
    let db = Database::new(&config.database_url).await?;

    // Выполнение миграций
    info!("Выполнение миграций...");
    db.run_migrations().await?;
    info!("Миграции выполнены успешно");

    // Без STRICT_AUTH в домашней сети входа нет: все работают от первого пользователя,
    // поэтому он нужен с первого запуска. Пароль случайный — для входа снаружи
    // заводится свой пользователь (или включается STRICT_AUTH)
    if !config.strict_auth && db.get_first_user().await?.is_none() {
        let password: String = rand::random::<[u8; 16]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        db.create_user("aurora", &auth::hash_password(&password)?)
            .await?;
        info!("Создан пользователь aurora для входа без пароля из локальной сети");
    }

    // Создание коллектора метрик
    let metrics = Arc::new(MetricsCollector::new());

    // Запуск периодического логирования метрик
    let metrics_logger = metrics.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60)); // Каждую минуту
        loop {
            interval.tick().await;
            if let Some(metrics_data) = metrics_logger.collect_all() {
                debug!(
                    "Метрики: память процесса={:.2}MB, CPU процесса={:.1}%, память системы={:.1}% ({:.2}MB/{:.2}MB), CPU системы={:.1}%",
                    metrics_data.process.memory_mb,
                    metrics_data.process.cpu_percent,
                    metrics_data.system.memory_percent,
                    metrics_data.system.used_memory_mb,
                    metrics_data.system.total_memory_mb,
                    metrics_data.system.cpu_usage_percent
                );
            }
        }
    });

    // Создание состояния приложения
    let config_arc = Arc::new(config.clone());
    let hls = HlsManager::new(config.ffmpeg_max_runs);

    // Периодическая очистка простаивающих HLS-сессий (temp-директории с сегментами)
    let hls_cleanup = hls.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5 * 60));
        loop {
            interval.tick().await;
            hls_cleanup
                .cleanup_idle(tokio::time::Duration::from_secs(15 * 60))
                .await;
        }
    });

    // Периодическая очистка истёкших pairing-кодов
    let pairing = PairingManager::new();
    let pairing_cleanup = pairing.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            pairing_cleanup.cleanup().await;
        }
    });

    let tmdb = tmdb::TmdbClient::from_config(&config).map(Arc::new);
    if tmdb.is_some() {
        info!("TMDB: обложки включены (язык {})", config.tmdb_language);
    } else {
        info!("TMDB: ключ не задан (TMDB_API_KEY), обложки отключены");
    }

    let state = AppState {
        config: config_arc.clone(),
        db,
        metrics: metrics.clone(),
        hls,
        scan_jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        pairing,
        tmdb,
    };

    // Догружаем метаданные, не полученные в прошлых запусках (например, ключ добавили позже)
    let enrich_state = state.clone();
    tokio::spawn(async move {
        server::enrich_pending_metadata(&enrich_state).await;
    });

    // Создание router
    let app = create_router(state);

    // Создание адреса для прослушивания
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Сервер запущен на http://{}", addr);
    if !config.strict_auth {
        info!("Вход без пароля из локальной сети включён (STRICT_AUTH=true — выключить)");
    }

    // Обработка graceful shutdown
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Не удалось установить обработчик сигнала");
        info!("Получен сигнал завершения, останавливаем сервер...");
    };

    // Запуск сервера
    // ConnectInfo нужен для входа без пароля из LAN
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
        .with_graceful_shutdown(shutdown_signal)
        .await
        .map_err(|e| anyhow::anyhow!("Ошибка сервера: {}", e))?;

    info!("Сервер остановлен");
    Ok(())
}
