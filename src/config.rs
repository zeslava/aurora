use anyhow::{Context, Result};
use std::path::PathBuf;

/// Конфигурация приложения
#[derive(Debug, Clone)]
pub struct Config {
    /// Порт для HTTP сервера
    pub port: u16,
    /// Пути к медиатекам
    pub media_paths: Vec<PathBuf>,
    /// Уровень логирования
    pub log_level: String,
    /// URL базы данных (SQLite)
    pub database_url: String,
    /// Ключ TMDB API (v3). Без него загрузка обложек отключена
    pub tmdb_api_key: Option<String>,
    /// Язык метаданных TMDB (например, "ru-RU")
    pub tmdb_language: String,
    /// Директория для кэша (обложки и прочее)
    pub cache_dir: PathBuf,
    /// Пароль обязателен всегда; иначе клиенты из локальной сети входят без него
    pub strict_auth: bool,
    /// Сколько HLS-ранов ffmpeg может идти одновременно на весь сервер
    pub ffmpeg_max_runs: usize,
    /// Ключ подписи JWT: JWT_SECRET или сгенерированный и сохранённый рядом с БД
    pub jwt_secret: String,
}

impl Config {
    /// Загружает конфигурацию из переменных окружения
    pub fn from_env() -> Result<Self> {
        // Загружаем .env файл если он существует
        dotenv::dotenv().ok();

        let port = std::env::var("PORT")
            .unwrap_or_else(|_| "4000".to_string())
            .parse::<u16>()
            .context("Неверный формат порта")?;

        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        // Получаем путь к БД из переменных окружения
        let database_url = std::env::var("DATABASE_URL")
            .or_else(|_| {
                std::env::var("DB_PATH").map(|path| {
                    if path.starts_with("sqlite:") {
                        path
                    } else {
                        format!("sqlite:{}", path)
                    }
                })
            })
            .unwrap_or_else(|_| "sqlite:./aurora.db".to_string());

        // Получаем пути к медиатекам из переменной MEDIA_PATHS (разделенные запятой)
        let media_paths_str = std::env::var("MEDIA_PATHS").unwrap_or_else(|_| String::new());

        let media_paths: Vec<PathBuf> = if media_paths_str.is_empty() {
            Vec::new()
        } else {
            media_paths_str
                .split(',')
                .map(|s| PathBuf::from(s.trim()))
                .collect()
        };

        // Пустая строка в TMDB_API_KEY равнозначна отсутствию ключа
        let tmdb_api_key = std::env::var("TMDB_API_KEY")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

        let tmdb_language = std::env::var("TMDB_LANGUAGE").unwrap_or_else(|_| "ru-RU".to_string());

        let cache_dir =
            PathBuf::from(std::env::var("CACHE_DIR").unwrap_or_else(|_| "./cache".to_string()));

        let strict_auth = std::env::var("STRICT_AUTH")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(false);

        let ffmpeg_max_runs = std::env::var("FFMPEG_MAX_RUNS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(crate::transcoder::DEFAULT_MAX_RUNS);

        let jwt_secret = match std::env::var("JWT_SECRET") {
            Ok(s) if !s.trim().is_empty() => s,
            _ => load_or_create_jwt_secret(&database_url)?,
        };

        Ok(Config {
            port,
            media_paths,
            log_level,
            database_url,
            tmdb_api_key,
            tmdb_language,
            cache_dir,
            strict_auth,
            ffmpeg_max_runs,
            jwt_secret,
        })
    }
}

/// Секрет живёт в файле `jwt_secret` рядом с SQLite: переживает рестарт (токены
/// не слетают) и попадает в тот же volume, что и БД. Для in-memory БД — одноразовый.
fn load_or_create_jwt_secret(database_url: &str) -> Result<String> {
    let db_path = database_url
        .trim_start_matches("sqlite:")
        .trim_start_matches("//")
        .split('?')
        .next()
        .unwrap_or_default();

    let generated = || -> String {
        rand::random::<[u8; 32]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };

    if db_path.is_empty() || db_path == ":memory:" {
        return Ok(generated());
    }

    let dir = std::path::Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let path = dir.join("jwt_secret");

    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return Ok(existing.to_string());
        }
    }

    let secret = generated();
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Не удалось создать каталог {}", dir.display()))?;
    std::fs::write(&path, &secret)
        .with_context(|| format!("Не удалось сохранить {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
    }
    eprintln!("JWT_SECRET не задан — сгенерирован и сохранён в {}", path.display());
    Ok(secret)
}
