use crate::auth::{self, create_token, hash_password, verify_password};
use crate::config::Config;
use crate::database::Database;
use crate::metadata::extract_metadata;
use crate::models::ContinueWatchingItem;
use crate::models::{Library, MediaFile, MediaItem, MediaType, PlaybackState};
use crate::scanner::scan_directory;
use crate::segments::{self, SegmentMap, StreamProfile};
use crate::tmdb::TmdbClient;
use crate::transcoder::{AudioTranscoder, HlsManager};
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Extension, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Json, Response},
    routing::{get, patch, post, put},
};
use bytes::Bytes;
use futures::stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::RwLock;
use tower::ServiceBuilder;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;

use crate::metrics::MetricsCollector;

/// Состояние приложения
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Database,
    pub metrics: Arc<MetricsCollector>,
    pub hls: HlsManager,
    pub scan_jobs: ScanJobs,
    pub pairing: crate::pairing::PairingManager,
    /// Клиент TMDB; None, если не задан TMDB_API_KEY
    pub tmdb: Option<Arc<TmdbClient>>,
}

/// Состояние фонового скана медиатеки
#[derive(Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Serialize)]
pub struct JobStatus {
    pub state: JobState,
    pub scanned: u64,
    pub total: u64,
    pub error: Option<String>,
}

pub type ScanJobs = Arc<RwLock<HashMap<Uuid, JobStatus>>>;

/// Ответ для health check
#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

/// Параметры запроса для списка медиа
#[derive(Deserialize)]
pub struct MediaQuery {
    #[serde(rename = "type")]
    pub media_type: Option<String>,
}

/// Запрос на регистрацию
#[derive(Deserialize)]
struct RegisterRequest {
    username: String,
    password: String,
}

/// Запрос на вход
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// Ответ с токеном
#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user_id: u64,
    username: String,
}

/// Запрос на обновление прогресса
#[derive(Deserialize)]
struct ProgressUpdateRequest {
    position: u64,
    completed: Option<bool>,
}

/// Создает и настраивает Axum router
pub fn create_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        // Свой SPA ходит same-origin, CORS нужен только ТВ-приложению с file:// (`Origin: null`).
        // Такой Origin бывает и у sandboxed iframe на чужом сайте, поэтому cross-origin
        // запросы всегда требуют токен — см. `trusted_lan`
        .allow_origin(AllowOrigin::exact(axum::http::HeaderValue::from_static(
            "null",
        )))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            header::RANGE,
        ])
        .expose_headers([
            header::CONTENT_RANGE,
            header::ACCEPT_RANGES,
            header::CONTENT_LENGTH,
            header::TRANSFER_ENCODING,
        ])
        .max_age(std::time::Duration::from_secs(3600));

    // Статика вшита в бинарь; STATIC_DIR — dev-override на чтение с диска.
    let static_override = std::env::var("STATIC_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir());

    let public = Router::new()
        .route("/health", get(health_handler))
        .route("/api/auth/register", post(register_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/lan", post(lan_login_handler))
        // Сопряжение устройств без клавиатуры: ТВ ещё не авторизован
        .route("/api/pair/start", post(pair_start_handler))
        .route("/api/pair/poll", post(pair_poll_handler));

    let protected = Router::new()
        .route("/api/metrics", get(metrics_handler))
        .route("/api/auth/refresh", post(refresh_handler))
        .route("/api/pair/confirm", post(pair_confirm_handler))
        .route("/api/libraries", get(libraries_handler))
        .route("/api/libraries/scan", post(scan_handler))
        .route("/api/scan", post(scan_start_handler))
        .route("/api/scan/:job_id", get(scan_status_handler))
        .route("/api/home", get(home_handler))
        .route("/api/media", get(media_list_handler))
        .route("/api/media/:id", get(media_detail_handler))
        .route("/api/media/:id", patch(update_media_handler))
        .route("/api/media/:id/stream", get(stream_handler))
        .route(
            "/api/media/:id/stream-transcoded",
            get(stream_transcoded_handler),
        )
        // Плейлист — на весь файл (VOD), URL постоянный на весь просмотр: позицию задаёт
        // плеер через `currentTime`/seek, сервер о ней не знает.
        .route(
            "/api/media/:id/hls/playlist.m3u8",
            get(hls_playlist_handler),
        )
        .route("/api/media/:id/hls/:segment", get(hls_segment_handler))
        // <img> не может слать Authorization — токен приходит в `?token=`
        .route("/api/media/:id/poster", get(poster_handler))
        .route("/api/media/:id/backdrop", get(backdrop_handler))
        .route(
            "/api/series/:id/seasons/:season/poster",
            get(season_poster_handler),
        )
        .route("/api/metadata/refresh", post(metadata_refresh_handler))
        .route("/api/media/:id/progress", get(get_progress_handler))
        .route("/api/media/:id/progress", put(update_progress_handler))
        .route("/api/series/:id/episodes", get(series_episodes_handler))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    let router = public
        .merge(protected)
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
        .with_state(state);

    // Раздача фронтенда (последним, чтобы не перехватывать API-маршруты).
    // Для SPA нужен fallback на index.html для всех несуществующих путей.
    match static_override {
        Some(dir) => router.fallback_service(
            ServeDir::new(&dir)
                .precompressed_gzip()
                .precompressed_br()
                .fallback(ServeFile::new(dir.join("index.html"))),
        ),
        None => router.fallback(crate::static_files::serve),
    }
}

/// Определяет пользователя запроса и кладёт его `Claims` в extensions.
/// Валидный токен (заголовок или `?token=` — `<video>`/`<img>` заголовки не шлют) →
/// его пользователь. Токена нет, но клиент из локальной сети и STRICT_AUTH выключен →
/// первый пользователь. Невалидный токен — 401 даже из LAN: клиент должен перелогиниться,
/// а не молча стать другим пользователем.
async fn require_auth(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, StatusCode> {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);

    let claims = match request_token(&req) {
        Some(token) => {
            auth::verify_token(&token, &state.config).map_err(|_| StatusCode::UNAUTHORIZED)?
        }
        None if trusted_lan(&state.config, peer, req.headers()) => {
            let (user_id, username) = state
                .db
                .get_first_user()
                .await
                .map_err(|e| {
                    tracing::error!("Ошибка при получении пользователя: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .ok_or(StatusCode::UNAUTHORIZED)?;
            auth::Claims {
                user_id,
                username,
                exp: 0,
            }
        }
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

fn request_token(req: &axum::extract::Request) -> Option<String> {
    auth::extract_token(req.headers().get(header::AUTHORIZATION)).or_else(|| {
        Query::<HashMap<String, String>>::try_from_uri(req.uri())
            .ok()?
            .0
            .remove("token")
    })
}

/// За reverse proxy на том же хосте все клиенты выглядят как loopback,
/// поэтому там доверие к LAN выключают через STRICT_AUTH=true.
/// Заголовки прокси (`X-Forwarded-For` и т.п.) не учитываются: закрывать доступ снаружи —
/// дело внешнего периметра, а не сервера.
/// Cross-origin запрос не доверенный даже из LAN: иначе любой сайт, открытый в домашнем
/// браузере, управлял бы сервером от имени первого пользователя
fn trusted_lan(config: &Config, peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    !config.strict_auth
        && peer.is_some_and(|addr| auth::is_lan(addr.ip()))
        && same_origin(headers)
}

/// Без `Origin` — навигация, `<video>`/`<img>` или не браузер; иначе хост из `Origin`
/// должен совпасть с `Host`. `Origin: null` (file://, sandboxed iframe) — не same-origin
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, host)| host);
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    origin_host.is_some() && origin_host == host
}

/// Health check endpoint
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

/// Метрики системы и процесса
async fn metrics_handler(
    State(state): State<AppState>,
) -> Result<Json<crate::metrics::Metrics>, StatusCode> {
    match state.metrics.collect_all() {
        Some(metrics) => Ok(Json(metrics)),
        None => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Регистрация пользователя
async fn register_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Первый пользователь регистрируется откуда угодно, дальше — только из доверенной
    // локальной сети или уже вошедшим пользователем: снаружи аккаунт себе не завести
    let has_users = state
        .db
        .get_first_user()
        .await
        .map_err(|e| {
            tracing::error!("Ошибка при проверке пользователей: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "message": "Внутренняя ошибка сервера" })),
            )
        })?
        .is_some();
    let authorized = auth::extract_token(headers.get(header::AUTHORIZATION))
        .is_some_and(|t| auth::verify_token(&t, &state.config).is_ok());
    if has_users && !authorized && !trusted_lan(&state.config, Some(addr), &headers) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "message": "Регистрация доступна только из локальной сети"
            })),
        ));
    }

    // Валидация имени пользователя
    if payload.username.is_empty() || payload.username.len() < 3 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "Имя пользователя должно быть не менее 3 символов"
            })),
        ));
    }

    // Валидация пароля
    if payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "Пароль должен быть не менее 6 символов"
            })),
        ));
    }

    // Проверяем, существует ли пользователь
    match state.db.get_user_by_username(&payload.username).await {
        Ok(Some(_)) => {
            return Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "message": "Пользователь с таким именем уже существует"
                })),
            ));
        }
        Err(e) => {
            tracing::error!("Ошибка при проверке пользователя: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "message": "Внутренняя ошибка сервера"
                })),
            ));
        }
        Ok(None) => {}
    }

    // Хешируем пароль
    let password_hash = hash_password(&payload.password).map_err(|e| {
        tracing::error!("Ошибка хеширования пароля: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "message": "Внутренняя ошибка сервера"
            })),
        )
    })?;

    // Создаем пользователя
    let user_id = state
        .db
        .create_user(&payload.username, &password_hash)
        .await
        .map_err(|e| {
            tracing::error!("Ошибка создания пользователя: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "message": "Внутренняя ошибка сервера"
                })),
            )
        })?;

    // Создаем токен
    let token = create_token(user_id, &payload.username, &state.config).map_err(|e| {
        tracing::error!("Ошибка создания токена: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "message": "Внутренняя ошибка сервера"
            })),
        )
    })?;

    Ok(Json(AuthResponse {
        token,
        user_id,
        username: payload.username,
    }))
}

/// Вход пользователя
async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Получаем пользователя
    let (user_id, password_hash) = match state.db.get_user_by_username(&payload.username).await {
        Ok(Some((id, hash))) => (id, hash),
        Ok(None) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "message": "Неверное имя пользователя или пароль"
                })),
            ));
        }
        Err(e) => {
            tracing::error!("Ошибка при получении пользователя: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "message": "Внутренняя ошибка сервера"
                })),
            ));
        }
    };

    // Проверяем пароль
    let password_valid = verify_password(&payload.password, &password_hash).map_err(|e| {
        tracing::error!("Ошибка проверки пароля: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "message": "Внутренняя ошибка сервера"
            })),
        )
    })?;

    if !password_valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "message": "Неверное имя пользователя или пароль"
            })),
        ));
    }

    // Создаем токен
    let token = create_token(user_id, &payload.username, &state.config).map_err(|e| {
        tracing::error!("Ошибка создания токена: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "message": "Внутренняя ошибка сервера"
            })),
        )
    })?;

    Ok(Json(AuthResponse {
        token,
        user_id,
        username: payload.username,
    }))
}

/// Продление токена: клиент зовёт при запуске, и пока устройством пользуются,
/// токен не истекает
async fn refresh_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<auth::Claims>,
) -> Result<Json<AuthResponse>, StatusCode> {
    let token = create_token(claims.user_id, &claims.username, &state.config).map_err(|e| {
        tracing::error!("Ошибка создания токена: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(AuthResponse {
        token,
        user_id: claims.user_id,
        username: claims.username,
    }))
}

/// Вход без пароля для клиентов из локальной сети — под первым пользователем
async fn lan_login_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<AuthResponse>, StatusCode> {
    if !trusted_lan(&state.config, Some(addr), &headers) {
        return Err(StatusCode::FORBIDDEN);
    }

    let (user_id, username) = state
        .db
        .get_first_user()
        .await
        .map_err(|e| {
            tracing::error!("Ошибка при получении пользователя: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::FORBIDDEN)?;

    let token = create_token(user_id, &username, &state.config).map_err(|e| {
        tracing::error!("Ошибка создания токена: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(AuthResponse {
        token,
        user_id,
        username,
    }))
}

/// Запрос на подтверждение pairing-кода
#[derive(Deserialize)]
struct PairConfirmRequest {
    code: String,
}

/// Запрос на опрос статуса сопряжения
#[derive(Deserialize)]
struct PairPollRequest {
    pairing_id: Uuid,
}

/// ТВ запрашивает новый pairing-код
async fn pair_start_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (pairing_id, code) = state.pairing.start().await;
    Json(serde_json::json!({
        "pairing_id": pairing_id.to_string(),
        "code": code,
    }))
}

/// Авторизованное устройство (телефон/десктоп) подтверждает код с ТВ
async fn pair_confirm_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<auth::Claims>,
    Json(payload): Json<PairConfirmRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let confirmed = state
        .pairing
        .confirm(&payload.code, claims.user_id, &claims.username)
        .await;

    if confirmed {
        Ok(Json(serde_json::json!({ "status": "confirmed" })))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "message": "Код не найден или истёк" })),
        ))
    }
}

/// ТВ опрашивает статус сопряжения по pairing_id
async fn pair_poll_handler(
    State(state): State<AppState>,
    Json(payload): Json<PairPollRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use crate::pairing::PollResult;
    match state.pairing.poll(&payload.pairing_id).await {
        PollResult::Pending => Ok(Json(serde_json::json!({ "status": "pending" }))),
        PollResult::Expired => Err(StatusCode::NOT_FOUND),
        PollResult::Confirmed { user_id, username } => {
            let token = create_token(user_id, &username, &state.config)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(Json(serde_json::json!({
                "status": "confirmed",
                "token": token,
                "user_id": user_id,
                "username": username,
            })))
        }
    }
}

/// Получить список библиотек
async fn libraries_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<Library>>, StatusCode> {
    match state.db.get_libraries().await {
        Ok(libraries) => Ok(Json(libraries)),
        Err(e) => {
            tracing::error!("Ошибка при получении библиотек: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Запустить сканирование медиатеки синхронно (блокирует до завершения)
async fn scan_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match perform_scan(&state, None).await {
        Ok(total_count) => Ok(Json(serde_json::json!({
            "status": "success",
            "count": total_count
        }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Запустить фоновое сканирование медиатеки, вернуть job_id немедленно
async fn scan_start_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let job_id = Uuid::now_v7();

    state.scan_jobs.write().await.insert(
        job_id,
        JobStatus {
            state: JobState::Running,
            scanned: 0,
            total: 0,
            error: None,
        },
    );

    let job_state = state.clone();
    tokio::spawn(async move {
        let result = perform_scan(&job_state, Some(job_id)).await;
        let mut jobs = job_state.scan_jobs.write().await;
        if let Some(status) = jobs.get_mut(&job_id) {
            match result {
                Ok(total) => {
                    status.state = JobState::Done;
                    status.scanned = total;
                    status.total = total;
                }
                Err(e) => {
                    status.state = JobState::Failed;
                    status.error = Some(e);
                }
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id.to_string() })),
    )
}

/// Получить статус фонового сканирования по job_id
async fn scan_status_handler(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobStatus>, StatusCode> {
    let jobs = state.scan_jobs.read().await;
    match jobs.get(&job_id) {
        Some(status) => Ok(Json(status.clone())),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Общая логика сканирования медиатек; если передан job_id — обновляет прогресс в ScanJobs
async fn perform_scan(state: &AppState, job_id: Option<Uuid>) -> Result<u64, String> {
    let mut total_count = 0u64;

    for media_path in &state.config.media_paths {
        tracing::info!("Сканирование директории: {:?}", media_path);

        // Определяем тип библиотеки по пути
        let path_str = media_path.to_string_lossy().to_lowercase();
        let library_type = if path_str.contains("tv") || path_str.contains("сериал") {
            MediaType::TvSeries
        } else {
            MediaType::Movie
        };

        // Создаем или получаем библиотеку
        let library_id = match state.db.get_libraries().await {
            Ok(libraries) => {
                // Ищем существующую библиотеку по пути
                let existing = libraries.iter().find(|lib| lib.path == *media_path);
                if let Some(lib) = existing {
                    lib.id
                } else {
                    // Создаем новую библиотеку
                    let name = media_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Библиотека")
                        .to_string();
                    match state
                        .db
                        .create_library(&name, media_path, &library_type)
                        .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!("Ошибка при создании библиотеки: {}", e);
                            return Err(format!("Ошибка при создании библиотеки: {}", e));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Ошибка при получении библиотек: {}", e);
                return Err(format!("Ошибка при получении библиотек: {}", e));
            }
        };

        // Сканируем директорию (блокирующие walkdir-операции — в отдельном пуле потоков)
        let media_path_owned = media_path.clone();
        let result = match tokio::task::spawn_blocking(move || scan_directory(&media_path_owned))
            .await
            .map_err(|e| format!("Паника при сканировании: {}", e))?
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Ошибка при сканировании {:?}: {}", media_path, e);
                return Err(format!("Ошибка при сканировании {:?}: {}", media_path, e));
            }
        };

        if let Some(job_id) = job_id {
            let total = result.movies.len() as u64
                + result
                    .series
                    .iter()
                    .map(|s| s.episodes.len() as u64)
                    .sum::<u64>();
            if let Some(status) = state.scan_jobs.write().await.get_mut(&job_id) {
                status.total += total;
            }
        }

        // Фильмы
        for movie in &result.movies {
            let Some(file_id) = insert_file_if_new(state, &movie.file_path).await else {
                // Файл уже в БД — но парсер имён мог с тех пор поумнеть
                match state
                    .db
                    .refresh_movie_title(&movie.file_path, &movie.title, movie.year)
                    .await
                {
                    Ok(true) => tracing::info!("Название обновлено: {}", movie.title),
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!("Не удалось обновить название {:?}: {}", movie.file_path, e)
                    }
                }
                continue;
            };
            match state
                .db
                .insert_media_item(
                    Some(library_id),
                    &movie.title,
                    movie.year,
                    &MediaType::Movie,
                    Some(file_id),
                )
                .await
            {
                Ok(_) => total_count += 1,
                Err(e) => tracing::warn!("Не удалось сохранить фильм {:?}: {}", movie.file_path, e),
            }
            if let Some(job_id) = job_id {
                if let Some(status) = state.scan_jobs.write().await.get_mut(&job_id) {
                    status.scanned += 1;
                }
            }
        }

        // Сериалы
        for series in &result.series {
            // Находим существующий сериал или создаём контейнер
            let series_id = match state
                .db
                .find_series_by_title(Some(library_id), &series.title)
                .await
            {
                Ok(Some(id)) => id,
                Ok(None) => match state
                    .db
                    .insert_media_item(
                        Some(library_id),
                        &series.title,
                        series.year,
                        &MediaType::TvSeries,
                        None,
                    )
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::warn!("Не удалось создать сериал {}: {}", series.title, e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Ошибка поиска сериала {}: {}", series.title, e);
                    continue;
                }
            };

            for episode in &series.episodes {
                let file_id = match insert_file_if_new(state, &episode.file_path).await {
                    Some(id) => id,
                    // Файл уже в БД — но парсер имён мог с тех пор поумнеть,
                    // и эпизод должен переехать к сериалу с новым названием
                    None => match relink_known_episode(state, &series_id, episode).await {
                        Ok(()) => continue,
                        Err(e) => {
                            tracing::warn!(
                                "Не удалось перепривязать эпизод {:?}: {}",
                                episode.file_path,
                                e
                            );
                            continue;
                        }
                    },
                };
                match state
                    .db
                    .insert_episode(
                        &series_id,
                        episode.season_number,
                        episode.episode_number,
                        episode.title.as_deref(),
                        file_id,
                    )
                    .await
                {
                    Ok(_) => total_count += 1,
                    Err(e) => {
                        tracing::warn!("Не удалось сохранить эпизод {:?}: {}", episode.file_path, e)
                    }
                }
                if let Some(job_id) = job_id {
                    if let Some(status) = state.scan_jobs.write().await.get_mut(&job_id) {
                        status.scanned += 1;
                    }
                }
            }
        }

        // Файлы, исчезнувшие с диска, убираем из БД (вместе с эпизодами и прогрессом)
        let present: std::collections::HashSet<PathBuf> = result
            .movies
            .iter()
            .map(|m| m.file_path.clone())
            .chain(
                result
                    .series
                    .iter()
                    .flat_map(|s| s.episodes.iter().map(|e| e.file_path.clone())),
            )
            .collect();
        match state.db.prune_missing_files(media_path, &present).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("Удалено исчезнувших файлов: {}", n),
            Err(e) => tracing::warn!("Не удалось подчистить {:?}: {}", media_path, e),
        }

        // Сериалы, чьи эпизоды переехали к контейнеру с новым названием
        if let Err(e) = state.db.cleanup_orphans().await {
            tracing::warn!("Не удалось подчистить осиротевшие элементы: {}", e);
        }

        tracing::info!("Сканирование {:?} завершено", media_path);
    }

    // Обложки догружаем в фоне: сетевые запросы не должны задерживать завершение скана
    if state.tmdb.is_some() {
        let enrich_state = state.clone();
        tokio::spawn(async move {
            enrich_tmdb(&enrich_state).await;
        });
    }

    Ok(total_count)
}

/// Публичная точка входа для догрузки метаданных при старте сервера
pub async fn enrich_pending_metadata(state: &AppState) {
    backfill_field_order(state).await;
    backfill_audio_tracks(state).await;
    enrich_tmdb(state).await;
}

/// Дозаполняет список аудиодорожек у файлов, попавших в БД до появления колонки.
///
/// Нужен ровно по той же причине, что и [`backfill_field_order`]: повторный скан известные
/// файлы не перечитывает, поэтому у всей уже собранной библиотеки список остался бы пустым —
/// и плеер не показал бы выбор перевода ни у одного фильма.
async fn backfill_audio_tracks(state: &AppState) -> u64 {
    let mut filled = 0u64;

    loop {
        let pending = match state.db.get_files_pending_audio_tracks(50).await {
            Ok(files) if files.is_empty() => break,
            Ok(files) => files,
            Err(e) => {
                tracing::warn!("Не удалось получить очередь audio_tracks: {}", e);
                break;
            }
        };

        for (file_id, path) in pending {
            // ffprobe — блокирующий вызов, в рантайме tokio ему не место.
            let probed = tokio::task::spawn_blocking(move || extract_metadata(&path)).await;

            // Пустой список у нечитаемого файла — это «пробовали»: иначе один битый файл
            // крутил бы бэкфилл вечно.
            let tracks = match probed {
                Ok(Ok(meta)) => meta.audio_tracks,
                Ok(Err(e)) => {
                    tracing::warn!("audio_tracks для файла {}: {}", file_id, e);
                    Vec::new()
                }
                Err(e) => {
                    tracing::warn!("Задача ffprobe для файла {} упала: {}", file_id, e);
                    Vec::new()
                }
            };

            if let Err(e) = state.db.set_audio_tracks(file_id, &tracks).await {
                tracing::warn!("Не удалось сохранить audio_tracks файла {}: {}", file_id, e);
                // Запись осталась NULL — следующий батч выберет её снова и уйдёт в петлю.
                return filled;
            }
            filled += 1;
        }
    }

    if filled > 0 {
        tracing::info!("audio_tracks: прочитаны у {} файлов", filled);
    }
    filled
}

/// Дозаполняет `field_order` у файлов, попавших в БД до появления этого признака.
///
/// Отдельная задача, а не часть скана: `insert_file_if_new` пропускает уже известные файлы,
/// поэтому повторное сканирование метаданные не перечитывает и признак остался бы пустым.
/// Работает пачками и переживает перезапуск: NULL означает «не пробовали», и после успешной
/// пробы там всегда что-то есть — в том числе "unknown", чтобы не гонять ffprobe по одному
/// и тому же файлу каждый старт.
async fn backfill_field_order(state: &AppState) -> u64 {
    let mut filled = 0u64;

    loop {
        let pending = match state.db.get_files_pending_field_order(50).await {
            Ok(files) if files.is_empty() => break,
            Ok(files) => files,
            Err(e) => {
                tracing::warn!("Не удалось получить очередь field_order: {}", e);
                break;
            }
        };

        for (file_id, path) in pending {
            // ffprobe — блокирующий вызов, в рантайме tokio ему не место.
            let probed = tokio::task::spawn_blocking(move || extract_metadata(&path)).await;

            let order = match probed {
                Ok(Ok(meta)) => meta.field_order.unwrap_or_else(|| "unknown".to_string()),
                // Файл мог исчезнуть или оказаться битым. Помечаем, чтобы очередь двигалась:
                // иначе один нечитаемый файл крутил бы бэкфилл вечно.
                Ok(Err(e)) => {
                    tracing::warn!("field_order для файла {}: {}", file_id, e);
                    "unknown".to_string()
                }
                Err(e) => {
                    tracing::warn!("Задача ffprobe для файла {} упала: {}", file_id, e);
                    "unknown".to_string()
                }
            };

            if let Err(e) = state.db.set_field_order(file_id, &order).await {
                tracing::warn!("Не удалось сохранить field_order файла {}: {}", file_id, e);
                // Запись осталась NULL — следующий батч выберет её снова и уйдёт в петлю.
                return filled;
            }
            filled += 1;
        }
    }

    if filled > 0 {
        tracing::info!("field_order: определён у {} файлов", filled);
    }
    filled
}

/// Догружает метаданные TMDB для элементов, которых ещё не касались.
/// Работает пачками, чтобы не держать в памяти всю библиотеку.
async fn enrich_tmdb(state: &AppState) -> u64 {
    let Some(tmdb) = state.tmdb.clone() else {
        return 0;
    };

    let mut enriched = 0u64;

    loop {
        let pending = match state.db.get_items_pending_tmdb(50).await {
            Ok(items) if items.is_empty() => break,
            Ok(items) => items,
            Err(e) => {
                tracing::warn!("Не удалось получить очередь TMDB: {}", e);
                break;
            }
        };

        for (id, title, year, media_type) in pending {
            match tmdb.search(&media_type, &title, year).await {
                Ok(Some(found)) => {
                    // Имена файлов включают id картинки на TMDB, поэтому при смене
                    // найденного фильма старые остаются на диске — убираем их сами
                    let previous = [
                        state.db.get_image_file(&id, "poster").await.ok().flatten(),
                        state
                            .db
                            .get_image_file(&id, "backdrop")
                            .await
                            .ok()
                            .flatten(),
                    ];

                    let poster = match &found.poster_path {
                        Some(path) => tmdb
                            .cache_poster(path, &id)
                            .await
                            .map_err(|e| tracing::warn!("Постер для {}: {}", title, e))
                            .ok(),
                        None => None,
                    };
                    let backdrop = match &found.backdrop_path {
                        Some(path) => tmdb
                            .cache_backdrop(path, &id)
                            .await
                            .map_err(|e| tracing::warn!("Фон для {}: {}", title, e))
                            .ok(),
                        None => None,
                    };

                    if let Err(e) = state
                        .db
                        .update_tmdb_metadata(
                            &id,
                            found.tmdb_id,
                            found.overview.as_deref(),
                            found.rating,
                            poster.as_deref(),
                            backdrop.as_deref(),
                        )
                        .await
                    {
                        tracing::warn!("Не удалось сохранить метаданные для {}: {}", title, e);
                    } else {
                        enriched += 1;

                        let current = [poster.as_deref(), backdrop.as_deref()];
                        for old in previous.iter().flatten() {
                            if !current.contains(&Some(old.as_str())) {
                                let _ = tokio::fs::remove_file(tmdb.images_dir().join(old)).await;
                            }
                        }

                        // Постеры сезонов есть только у сериала на TMDB; тип
                        // элемента в библиотеке для этого не показателен —
                        // мини-сериал одним файлом лежит среди фильмов
                        if media_type == MediaType::TvSeries
                            && found.media_type == MediaType::TvSeries
                        {
                            enrich_season_posters(state, &tmdb, &id, found.tmdb_id).await;
                        }
                    }
                }
                Ok(None) => {
                    tracing::debug!("TMDB: ничего не найдено для «{}»", title);
                    let _ = state.db.mark_tmdb_synced(&id).await;
                }
                Err(e) => {
                    // Сетевая ошибка — не помечаем как обработанное, попробуем в следующий раз
                    tracing::warn!("Ошибка запроса к TMDB для «{}»: {}", title, e);
                    return enriched;
                }
            }
        }
    }

    tracing::info!("TMDB: обновлено метаданных — {}", enriched);
    enriched
}

/// Догружает постеры сезонов сериала — только тех, для которых на диске есть эпизоды.
/// Сезон сопоставляется с TMDB по номеру: названий у сезонов в библиотеке нет.
async fn enrich_season_posters(
    state: &AppState,
    tmdb: &crate::tmdb::TmdbClient,
    series_id: &str,
    tmdb_id: u64,
) {
    let local = match state.db.get_season_numbers(series_id).await {
        Ok(seasons) if !seasons.is_empty() => seasons,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("Не удалось получить сезоны {}: {}", series_id, e);
            return;
        }
    };

    let seasons = match tmdb.seasons(tmdb_id).await {
        Ok(seasons) => seasons,
        Err(e) => {
            tracing::warn!("Сезоны TMDB для {}: {}", series_id, e);
            return;
        }
    };

    for season in seasons.iter().filter(|s| local.contains(&s.season_number)) {
        let Some(path) = season.poster_path.as_deref() else {
            continue;
        };

        let previous = state
            .db
            .get_season_image_file(series_id, season.season_number)
            .await
            .ok()
            .flatten();

        match tmdb
            .cache_season_poster(path, series_id, season.season_number)
            .await
        {
            Ok(file) => {
                if let Err(e) = state
                    .db
                    .upsert_season_image(series_id, season.season_number, &file)
                    .await
                {
                    tracing::warn!("Не удалось сохранить постер сезона: {}", e);
                } else if let Some(old) = previous.filter(|old| *old != file) {
                    let _ = tokio::fs::remove_file(tmdb.images_dir().join(old)).await;
                }
            }
            Err(e) => tracing::warn!(
                "Постер сезона {} для {}: {}",
                season.season_number,
                series_id,
                e
            ),
        }
    }
}

/// Отдаёт закэшированную обложку с диска
async fn serve_cached_image(
    state: &AppState,
    id: &str,
    kind: &str,
) -> Result<Response<Body>, StatusCode> {
    let file_name = state
        .db
        .get_image_file(id, kind)
        .await
        .map_err(|e| {
            tracing::error!("Ошибка получения обложки: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    image_response(state, &file_name).await
}

/// Читает файл из кэша изображений и заворачивает в ответ
async fn image_response(state: &AppState, file_name: &str) -> Result<Response<Body>, StatusCode> {
    let tmdb = state.tmdb.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    let path = tmdb.images_dir().join(file_name);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let content_type = if file_name.ends_with(".png") {
        "image/png"
    } else if file_name.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Постер фильма или сериала
async fn poster_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response<Body>, StatusCode> {
    serve_cached_image(&state, &id, "poster").await
}

/// Фоновый кадр фильма или сериала
async fn backdrop_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response<Body>, StatusCode> {
    serve_cached_image(&state, &id, "backdrop").await
}

/// Постер сезона сериала
async fn season_poster_handler(
    State(state): State<AppState>,
    Path((id, season)): Path<(String, u32)>,
) -> Result<Response<Body>, StatusCode> {
    let file_name = state
        .db
        .get_season_image_file(&id, season)
        .await
        .map_err(|e| {
            tracing::error!("Ошибка получения постера сезона: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    image_response(&state, &file_name).await
}

/// Перезапрашивает метаданные TMDB для всей библиотеки (в фоне)
async fn metadata_refresh_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.tmdb.is_none() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let reset = state.db.reset_tmdb_sync().await.map_err(|e| {
        tracing::error!("Не удалось сбросить синхронизацию TMDB: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let job_state = state.clone();
    tokio::spawn(async move {
        enrich_tmdb(&job_state).await;
    });

    Ok(Json(
        serde_json::json!({ "status": "started", "queued": reset }),
    ))
}

/// Извлекает метаданные и вставляет новый media_files. Возвращает None, если файл
/// уже есть в БД или произошла ошибка (тогда файл пропускается).
/// Файл-эпизод уже в БД: переносим его к сериалу с актуальным названием
/// (или заводим эпизод, если строки в `episodes` почему-то нет)
async fn relink_known_episode(
    state: &AppState,
    series_id: &str,
    episode: &crate::scanner::ScannedEpisode,
) -> anyhow::Result<()> {
    let Some(file_id) = state.db.media_file_id(&episode.file_path).await? else {
        return Ok(());
    };

    if state.db.episode_exists_for_file(file_id).await? {
        if state
            .db
            .relink_episode(
                file_id,
                series_id,
                episode.season_number,
                episode.episode_number,
            )
            .await?
        {
            tracing::info!("Эпизод перепривязан: {:?}", episode.file_path);
        }
    } else {
        state
            .db
            .insert_episode(
                series_id,
                episode.season_number,
                episode.episode_number,
                episode.title.as_deref(),
                file_id,
            )
            .await?;
    }

    Ok(())
}

async fn insert_file_if_new(state: &AppState, file_path: &PathBuf) -> Option<u64> {
    match state.db.media_file_exists(file_path).await {
        Ok(true) => {
            tracing::debug!("Файл уже существует в БД: {:?}", file_path);
            return None;
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("Ошибка проверки файла {:?}: {}", file_path, e);
            return None;
        }
    }

    let mut file = MediaFile {
        id: 0,
        file_path: file_path.clone(),
        duration: None,
        resolution: None,
        video_codec: None,
        audio_codec: None,
        video_bitrate: None,
        pix_fmt: None,
        field_order: None,
        audio_tracks: Vec::new(),
    };

    match extract_metadata(file_path) {
        Ok(metadata) => {
            file.duration = metadata.duration;
            file.resolution = metadata.resolution();
            file.video_codec = metadata.video_codec;
            file.audio_codec = metadata.audio_codec;
            file.video_bitrate = metadata.video_bitrate;
            file.pix_fmt = metadata.pix_fmt;
            file.field_order = metadata.field_order;
            file.audio_tracks = metadata.audio_tracks;
        }
        Err(e) => tracing::warn!("Не удалось извлечь метаданные для {:?}: {}", file_path, e),
    }

    match state.db.insert_media_file(&file).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("Не удалось сохранить файл {:?}: {}", file_path, e);
            None
        }
    }
}

/// Получить список медиа
#[derive(Serialize)]
struct HomeResponse {
    continue_watching: Vec<ContinueWatchingItem>,
}

/// Главный экран: «Продолжить просмотр» (по текущему пользователю).
async fn home_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<auth::Claims>,
) -> Result<Json<HomeResponse>, StatusCode> {
    let continue_watching = state
        .db
        .get_continue_watching(claims.user_id, 20)
        .await
        .map_err(|e| {
            tracing::error!("Ошибка получения «продолжить просмотр»: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(HomeResponse { continue_watching }))
}

async fn media_list_handler(
    State(state): State<AppState>,
    Query(params): Query<MediaQuery>,
) -> Result<Json<Vec<MediaItem>>, StatusCode> {
    let media_type = if let Some(type_str) = params.media_type {
        match type_str.to_lowercase().as_str() {
            "movie" => Some(MediaType::Movie),
            "tv" | "tvseries" | "series" => Some(MediaType::TvSeries),
            _ => None,
        }
    } else {
        None
    };

    match state.db.get_media_items(media_type.as_ref()).await {
        Ok(items) => Ok(Json(items)),
        Err(e) => {
            tracing::error!("Ошибка при получении медиа элементов: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Получить детали медиа по ID
async fn media_detail_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MediaItem>, StatusCode> {
    match state.db.get_media_item_by_id(&id).await {
        Ok(Some(item)) => Ok(Json(item)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Ошибка при получении медиа элемента: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Максимальный размер чанка для потоковой передачи (10 МБ)
/// Потоковая передача медиафайла
async fn stream_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(_query): Query<StreamQuery>, // Принимаем токен, но не используем (эндпоинт публичный)
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // id может быть как фильмом, так и эпизодом — резолвим файл единообразно
    let file_path = state
        .db
        .resolve_media_file(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?
        .file_path;

    let file = File::open(&file_path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Получаем размер файла
    let file_size = file
        .metadata()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .len();

    // Обрабатываем Range запрос.
    // Отдаём запрошенный диапазон целиком (при `bytes=N-` — до конца файла).
    // Поток читается кусками по 64 КБ, поэтому память не расходуется на весь диапазон,
    // а браузер тянет данные по одному соединению без периодических дозапросов.
    let range = headers
        .get(header::RANGE)
        .and_then(|h| h.to_str().ok())
        .map_or(ByteRange::Whole, |r| parse_range(r, file_size));
    // Конец исключительный: у пустого файла включительного конца нет
    let (start, end_excl) = match range {
        ByteRange::Part(start, end) => (start, end + 1),
        ByteRange::Whole => (0, file_size),
        ByteRange::Unsatisfiable => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{file_size}"))
                .body(Body::empty())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let partial = start > 0 || end_excl < file_size;

    let content_length = end_excl - start;

    // Перемещаемся к нужной позиции в файле
    let mut file = file;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Создаем поток для чтения файла по частям
    // Читаем файл по частям размером 64 КБ
    const CHUNK_SIZE: usize = 64 * 1024;
    let remaining = content_length;

    let stream = stream::unfold(
        (file, remaining),
        move |(mut file, mut remaining)| async move {
            if remaining == 0 {
                return None;
            }

            let chunk_size = std::cmp::min(CHUNK_SIZE as u64, remaining) as usize;
            let mut buffer = vec![0u8; chunk_size];

            match file.read(&mut buffer).await {
                Ok(0) => None, // EOF
                Ok(n) => {
                    buffer.truncate(n);
                    remaining = remaining.saturating_sub(n as u64);
                    Some((Ok(Bytes::from(buffer)), (file, remaining)))
                }
                Err(e) => Some((Err(e), (file, remaining))),
            }
        },
    );

    // Определяем MIME тип
    let content_type = get_mime_type(&file_path);

    // Формируем ответ
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, content_length)
        .header(header::ACCEPT_RANGES, "bytes");

    if partial {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end_excl - 1, file_size),
        );
    }

    Ok(response
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

/// Параметры запроса для стрима (токен через query)
#[derive(Deserialize)]
struct StreamQuery {
    /// Принимается от `<video>`, но намеренно не проверяется — стримы публичные
    #[allow(dead_code)]
    token: Option<String>,
    /// Время начала в секундах (для транскодированного потока)
    start: Option<u64>,
    /// Номер аудиодорожки среди аудиопотоков файла
    audio: Option<u32>,
}

/// Потоковая передача с транскодированием аудио (AC3 → AAC)
async fn stream_transcoded_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // Резолвим файл (фильм или эпизод)
    let media_file = state
        .db
        .resolve_media_file(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Проверяем, нужен ли транскодинг (AC3, DTS и т.д.)
    // Браузеры не поддерживают AC3/DTS напрямую, поэтому транскодинг необходим
    let audio_index = resolve_audio_index(&media_file, query.audio);
    let needs_transcoding = needs_audio_transcoding(&audio_codec_of(&media_file, audio_index));

    if !needs_transcoding {
        // Если транскодинг не нужен, используем обычный поток с Range запросами
        return stream_handler(State(state), Path(id.clone()), Query(query), headers).await;
    }

    // Для транскодированного потока ограничиваем длительность для предотвращения загрузки всего файла
    // Браузер будет запрашивать новые чанки через параметр start при перемотке

    let file_path = media_file.file_path;

    // Определяем начальное время для транскодирования
    // Приоритет: query параметр start > Range заголовок (конвертация байтов в секунды)
    let start_time = if let Some(start_sec) = query.start {
        // Используем query параметр, если он указан
        start_sec
    } else if let Some(range_str) = headers.get(header::RANGE).and_then(|h| h.to_str().ok()) {
        // Пытаемся использовать Range заголовок и конвертировать байты в секунды
        let file_size = tokio::fs::metadata(&file_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .len();

        match parse_range(range_str, file_size) {
            ByteRange::Part(start_byte, _) => {
                range_start_seconds(start_byte, file_size, media_file.duration)
            }
            ByteRange::Whole | ByteRange::Unsatisfiable => 0,
        }
    } else {
        0
    };

    // Ограничиваем длительность транскодирования для предотвращения загрузки всего файла
    // Транскодируем небольшие чанки (2 минуты) для потоковой передачи
    const TRANSCODE_CHUNK_DURATION: u64 = 120; // 2 минуты в секундах

    tracing::info!(
        "Транскодирование аудио для медиа ID {} (кодек: {:?}, начало: {} сек, длительность чанка: {} сек)",
        id,
        media_file.audio_codec,
        start_time,
        TRANSCODE_CHUNK_DURATION
    );

    // Создаем транскодер с ограниченной длительностью
    let transcoder = AudioTranscoder::new(
        &file_path,
        start_time,
        Some(TRANSCODE_CHUNK_DURATION),
        audio_index,
    )
    .await
    .map_err(|e| {
        tracing::error!("Ошибка создания транскодера: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Определяем MIME тип (MP4 для потоковой передачи)
    let content_type = "video/mp4";

    // Формируем ответ
    // Для транскодированного потока используем chunked transfer
    // НЕ ограничиваем размер - ffmpeg сам контролирует через -t (duration)
    // Обрезка потока по байтам ломает MP4 контейнер
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::TRANSFER_ENCODING, "chunked")
        // Отключаем кэширование для транскодированного потока
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(header::PRAGMA, "no-cache")
        .header(header::EXPIRES, "0");

    Ok(response
        .body(Body::from_stream(transcoder))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

/// Белый список аудиокодеков, которые можно класть в fMP4 без перекодирования.
/// Всё остальное (ac3/dts/truehd/eac3, а также mp3 — `mp4a.6b` не поддерживается в MSE)
/// перекодируется в AAC.
const BROWSER_AUDIO_CODECS: [&str; 2] = ["aac", "opus"];

/// Белый список видеокодеков для `-c:v copy` в fMP4. mpeg4 (XviD/DivX), mpeg2, vc1, hevc, vp9
/// в MSE не декодируются, поэтому всё, чего нет в списке, идёт в H.264.
const BROWSER_VIDEO_CODECS: [&str; 2] = ["h264", "avc"];

/// Браузер декодирует из fMP4 только AAC/Opus — для остального нужен транскодинг аудио.
/// Неизвестный кодек (нет метаданных) оставляем как есть — иначе жгли бы CPU на всей библиотеке.
/// Номер запрошенной дорожки, приведённый к тому, что есть в файле. Неизвестный номер
/// (устаревший плейлист, ручной запрос) — это дорожка 0, а не ошибка: лучше отдать звук
/// по умолчанию, чем сломать воспроизведение.
fn resolve_audio_index(media_file: &MediaFile, requested: Option<u32>) -> u32 {
    let count = media_file.audio_tracks.len() as u32;
    match requested {
        Some(i) if i < count => i,
        _ => 0,
    }
}

/// Кодек конкретной дорожки. У файлов, отсканированных до появления `audio_tracks`,
/// список пуст — там остаётся единственный известный `audio_codec`.
fn audio_codec_of(media_file: &MediaFile, index: u32) -> Option<String> {
    media_file
        .audio_tracks
        .iter()
        .find(|t| t.index == index)
        .and_then(|t| t.codec.clone())
        .or_else(|| media_file.audio_codec.clone())
}

fn needs_audio_transcoding(audio_codec: &Option<String>) -> bool {
    audio_codec
        .as_ref()
        .map(|codec| {
            let codec_lower = codec.to_lowercase();
            !BROWSER_AUDIO_CODECS
                .iter()
                .any(|&c| codec_lower.contains(c))
        })
        .unwrap_or(false)
}

/// Браузер (в т.ч. webOS) играет из fMP4 только H.264 8-bit — остальное перекодируем.
fn needs_video_transcoding(
    video_codec: &Option<String>,
    pix_fmt: &Option<String>,
    field_order: &Option<String>,
) -> bool {
    let codec_needs_transcode = video_codec
        .as_ref()
        .map(|codec| {
            let codec_lower = codec.to_lowercase();
            !BROWSER_VIDEO_CODECS
                .iter()
                .any(|&c| codec_lower.contains(c))
        })
        .unwrap_or(false);

    let is_10bit = pix_fmt
        .as_ref()
        .map(|fmt| fmt.to_lowercase().contains("10le") || fmt.to_lowercase().contains("10be"))
        .unwrap_or(false);

    codec_needs_transcode || is_10bit || is_interlaced(field_order)
}

/// Чересстрочный источник. ffprobe отдаёт "progressive" для прогрессивного, "tt"/"bb"/"tb"/"bt"
/// для полей и "unknown", когда определить не смог. Неизвестное считаем прогрессивным: иначе
/// перекодировали бы всю библиотеку из-за отсутствующих метаданных.
///
/// Почему это вообще решает copy|transcode: чересстрочный H.264 проходит белый список кодеков
/// и уходит в `-c:v copy`, но MSE поля не декодирует — картинка идёт рывками при копеечном
/// битрейте. Замер: DVB-рип 720x576 tt, 3.6 Mbit/s. См. docs/decisions/0005.
fn is_interlaced(field_order: &Option<String>) -> bool {
    field_order
        .as_ref()
        .map(|f| matches!(f.to_lowercase().as_str(), "tt" | "bb" | "tb" | "bt"))
        .unwrap_or(false)
}

/// Достаёт карту сегментов из кэша БД. Карта, не подходящая профилю (например, сохранённая
/// в старом формате или другой версией правил нарезки), считается промахом кэша: отдать сетку
/// в `-c:v copy` — тихий рассинхрон, а не деградация.
async fn cached_segment_map(
    state: &AppState,
    file_id: u64,
    profile: StreamProfile,
) -> Result<Option<SegmentMap>, StatusCode> {
    let Some(json) = state
        .db
        .get_segment_map(file_id, profile.key())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Ok(None);
    };

    Ok(SegmentMap::from_json(&json).filter(|map| {
        let ok = map.matches(profile);
        if !ok {
            tracing::warn!(
                "Карта сегментов файла {} не подходит профилю {} — строю заново",
                file_id,
                profile.key()
            );
        }
        ok
    }))
}

/// Резолвит файл, выводит профиль потока и достаёт карту сегментов — из кэша БД,
/// а при промахе строит через ffprobe и кэширует.
async fn segment_map_for(
    state: &AppState,
    id: &str,
    audio: Option<u32>,
) -> Result<(MediaFile, StreamProfile, SegmentMap), StatusCode> {
    let media_file = state
        .db
        .resolve_media_file(id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let audio_index = resolve_audio_index(&media_file, audio);
    let mut profile = StreamProfile {
        transcode_video: needs_video_transcoding(
            &media_file.video_codec,
            &media_file.pix_fmt,
            &media_file.field_order,
        ),
        transcode_audio: needs_audio_transcoding(&audio_codec_of(&media_file, audio_index)),
        deinterlace: is_interlaced(&media_file.field_order),
        audio_index,
    };

    if let Some(map) = cached_segment_map(state, media_file.id, profile).await? {
        return Ok((media_file, profile, map));
    }

    let Some(duration) = media_file.duration else {
        tracing::error!(
            "Не удалось построить карту сегментов для {}: неизвестна длительность файла",
            id
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };

    let map =
        match segments::build_segment_map(&media_file.file_path, duration as f64, profile).await {
            Ok(map) => map,
            // Карту для `-c:v copy` построить не вышло (нет индекса keyframes) — переключаем файл
            // на перекодирование видео: там границы сегментов задаём мы, а не источник.
            Err(e) if !profile.transcode_video => {
                tracing::warn!(
                    "Карта сегментов для {} в copy-профиле недоступна ({}) — перекодируем видео",
                    id,
                    e
                );
                profile.transcode_video = true;
                if let Some(map) = cached_segment_map(state, media_file.id, profile).await? {
                    return Ok((media_file, profile, map));
                }
                segments::build_segment_map(&media_file.file_path, duration as f64, profile)
                    .await
                    .map_err(|e| {
                        tracing::error!("Не удалось построить карту сегментов для {}: {}", id, e);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?
            }
            Err(e) => {
                tracing::error!("Не удалось построить карту сегментов для {}: {}", id, e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    state
        .db
        .save_segment_map(media_file.id, profile.key(), &map.to_json())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((media_file, profile, map))
}

/// Отдаёт VOD-плейлист на весь файл. Рендерится из карты сегментов, ничего не ждёт —
/// сегменты при этом ещё не существуют, они генерируются лениво по запросу.
async fn hls_playlist_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    let requested_audio = params.get("audio").and_then(|a| a.parse::<u32>().ok());
    let (_media_file, profile, map) = segment_map_for(&state, &id, requested_audio).await?;

    tracing::info!(
        "HLS плейлист: media={id} профиль={} сегментов={}",
        profile.cache_key(),
        map.count()
    );

    let mut playlist = map.to_playlist(profile);
    if let Some(token) = params.get("token") {
        playlist = with_token(&playlist, token);
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(playlist))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

/// Относительные URI сегментов не наследуют query плейлиста, а нативный HLS-плеер
/// не даёт добавить заголовок — токен дописывается в каждый URI.
/// URI сегментов и `init.mp4` всегда уже содержат `?` (см. `segments::segment_uri`).
fn with_token(playlist: &str, token: &str) -> String {
    let token = urlencoding_component(token);
    playlist
        .lines()
        .map(|line| {
            if line.starts_with("#EXT-X-MAP:URI=\"") {
                line.replacen('?', &format!("?token={token}&"), 1)
            } else if !line.is_empty() && !line.starts_with('#') {
                format!("{line}&token={token}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// JWT состоит из base64url и точек, но токен пришёл от клиента — экранируем всё прочее,
/// чтобы он не смог дописать в плейлист свои параметры или строки
fn urlencoding_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Отдаёт сегмент (или `init.mp4`), генерируя его лениво при первом обращении.
async fn hls_segment_handler(
    State(state): State<AppState>,
    Path((id, segment)): Path<(String, String)>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, StatusCode> {
    let requested_audio = params.get("a").and_then(|a| a.parse::<u32>().ok());
    let (media_file, profile, map) = segment_map_for(&state, &id, requested_audio).await?;

    // Плейлист помечен профилем, при котором он был отрендерен. Расхождение означает, что
    // у плеера на руках устаревший VOD-манифест (он их не перезапрашивает) — отвечаем 404,
    // чтобы клиент перезагрузил плейлист, а не ретраил мёртвые URL.
    // Дорожка проверяется тем же способом: `a` из плейлиста, не совпавшая с выбранной
    // (её нет в файле), означала бы, что сегменты придут с чужим звуком.
    let stale_marker = params
        .get("p")
        .filter(|p| p.as_str() != profile.key())
        .cloned()
        .or_else(|| {
            requested_audio
                .filter(|a| *a != profile.audio_index)
                .map(|a| format!("audio={a}"))
        });
    if let Some(stale) = stale_marker {
        tracing::warn!(
            "HLS устаревший плейлист: media={id} {segment} маркер={stale}, текущий={}",
            profile.cache_key()
        );
        return Err(StatusCode::NOT_FOUND);
    }

    // Входящий запрос логируем отдельно от результата: строка запроса без парной строки
    // «готов»/«таймаут» из transcoder означает, что клиент оборвал соединение сам (у hls.js
    // на первый байт фрагмента отведено 10с по умолчанию).
    tracing::info!(
        "HLS запрос: media={id} {segment} профиль={}",
        profile.cache_key()
    );

    let (bytes, content_type) = if segment == "init.mp4" {
        // init есть только у fMP4-профиля; в TS-плейлисте нет `EXT-X-MAP` и такого запроса
        // быть не должно.
        if !profile.uses_fmp4() {
            return Err(StatusCode::NOT_FOUND);
        }
        let bytes = state
            .hls
            .init_segment(media_file.id, &media_file.file_path, profile, &map)
            .await
            .map_err(|e| {
                tracing::error!("Не удалось сгенерировать init.mp4 для {}: {}", id, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        (bytes, "video/mp4")
    } else {
        // Имя не разбирается под текущий профиль, а маркера не было (старый плейлист без `p`
        // или чужой запрос). Для клиента это то же самое «перезагрузи плейлист», что и выше.
        let index = segments::parse_segment_name(&segment, profile).ok_or(StatusCode::NOT_FOUND)?;
        if index >= map.count() {
            return Err(StatusCode::NOT_FOUND);
        }
        let bytes = state
            .hls
            .segment(media_file.id, &media_file.file_path, profile, &map, index)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Не удалось сгенерировать сегмент {} для {}: {}",
                    segment,
                    id,
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let content_type = if profile.uses_fmp4() {
            "video/iso.segment"
        } else {
            "video/mp2t"
        };
        (bytes, content_type)
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        // Содержимое сегмента детерминировано его номером — кэшировать можно навсегда.
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

/// Конвертирует байтовую позицию Range в секунды для seek транскодера.
/// Используется, когда клиент промотал через байтовый Range, а не через `?start=`.
/// Если длительность неизвестна — оценивает её по среднему битрейту ~5 Мбит/с.
fn range_start_seconds(start_byte: u64, file_size: u64, duration: Option<u64>) -> u64 {
    if file_size == 0 {
        return 0;
    }
    // Приблизительная длительность по среднему битрейту, если метаданных нет.
    let duration = duration.unwrap_or_else(|| (file_size * 8) / (5 * 1_000_000));
    if duration == 0 {
        return 0;
    }
    ((start_byte as f64 / file_size as f64) * duration as f64) as u64
}

/// Парсит Range заголовок
#[derive(Debug, PartialEq)]
enum ByteRange {
    /// Заголовок не разобран — по RFC 9110 его игнорируют и отдают файл целиком
    Whole,
    /// Включительные границы
    Part(u64, u64),
    /// Синтаксически верный, но вне файла → 416
    Unsatisfiable,
}

/// Разбор одиночного `Range: bytes=…` по RFC 9110: конец за пределами файла урезается,
/// `bytes=-N` — последние N байт
fn parse_range(range_str: &str, file_size: u64) -> ByteRange {
    let Some((first, last)) = range_str
        .strip_prefix("bytes=")
        .and_then(|r| r.split_once('-'))
    else {
        return ByteRange::Whole;
    };
    let parse = |s: &str| s.trim().parse::<u64>().ok();

    match (first.trim().is_empty(), last.trim().is_empty()) {
        (true, true) => ByteRange::Whole,
        (true, false) => match parse(last) {
            None => ByteRange::Whole,
            Some(0) => ByteRange::Unsatisfiable,
            Some(_) if file_size == 0 => ByteRange::Unsatisfiable,
            Some(n) => ByteRange::Part(file_size.saturating_sub(n), file_size - 1),
        },
        (false, open_end) => {
            let Some(start) = parse(first) else {
                return ByteRange::Whole;
            };
            let end = if open_end {
                u64::MAX
            } else {
                match parse(last) {
                    Some(end) => end,
                    None => return ByteRange::Whole,
                }
            };
            if start > end {
                ByteRange::Whole
            } else if start >= file_size {
                ByteRange::Unsatisfiable
            } else {
                ByteRange::Part(start, end.min(file_size - 1))
            }
        }
    }
}

/// Определяет MIME тип по расширению файла
fn get_mime_type(path: &PathBuf) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mp4") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Получить прогресс просмотра
async fn get_progress_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(claims): Extension<auth::Claims>,
) -> Result<Response, StatusCode> {
    let user_id = claims.user_id;

    // Резолвим файл по id (фильм или эпизод) — прогресс хранится на уровне файла
    let file = state
        .db
        .resolve_media_file(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // «Ещё не смотрели» — это не ошибка, а нормальный ответ: 404 здесь означал бы, что нет
    // самого медиа. Отдаём `null`, иначе каждый непросмотренный айтем в библиотеке засоряет
    // консоль клиента красной строкой (на webOS её видно только через ares-inspect).
    let progress = match state.db.get_playback_state(user_id, file.id, id).await {
        Ok(Some(progress)) => Some(progress),
        Ok(None) => None,
        Err(e) => {
            tracing::error!("Ошибка при получении прогресса: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Позиция меняется каждые несколько секунд и приходит с разных устройств —
    // закэшированный ответ браузера показал бы устаревшую позицию при resume.
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(
            serde_json::to_vec(&progress).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        ))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Обновить прогресс просмотра
async fn update_progress_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(claims): Extension<auth::Claims>,
    Json(payload): Json<ProgressUpdateRequest>,
) -> Result<Json<PlaybackState>, StatusCode> {
    let user_id = claims.user_id;

    // Резолвим файл по id (фильм или эпизод) — прогресс хранится на уровне файла
    let file = state
        .db
        .resolve_media_file(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let progress = PlaybackState {
        user_id,
        media_id: id,
        position: payload.position,
        completed: payload.completed.unwrap_or(false),
        last_watched: Some(chrono::Utc::now()),
    };

    state
        .db
        .upsert_playback_state(
            user_id,
            file.id,
            progress.position,
            progress.completed,
            progress.last_watched,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(progress))
}

#[derive(Debug, Deserialize)]
struct UpdateMediaRequest {
    /// Пустая строка/пробелы = снять пользовательское название
    custom_title: Option<String>,
}

/// Задать или снять пользовательское название фильма/сериала.
/// Сбрасывает отметку синхронизации TMDB и запускает обогащение: метаданные
/// и обложки подтянутся уже по новому названию.
async fn update_media_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateMediaRequest>,
) -> Result<Json<MediaItem>, StatusCode> {
    let custom_title = payload
        .custom_title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    let updated = state
        .db
        .set_custom_title(&id, custom_title.as_deref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !updated {
        return Err(StatusCode::NOT_FOUND);
    }

    // Постеры сезонов привязаны к прежнему сериалу на TMDB: по новому названию
    // может найтись другой — старые обложки должны уйти вместе с ним
    if let Some(tmdb) = state.tmdb.as_ref() {
        match state.db.clear_season_images(&id).await {
            Ok(files) => {
                for file in files {
                    let _ = tokio::fs::remove_file(tmdb.images_dir().join(file)).await;
                }
            }
            Err(e) => tracing::warn!("Не удалось убрать постеры сезонов {}: {}", id, e),
        }
    }

    let item = state
        .db
        .get_media_item_by_id(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let enrich_state = state.clone();
    tokio::spawn(async move {
        enrich_tmdb(&enrich_state).await;
    });

    Ok(Json(item))
}

/// Получить эпизоды сериала
async fn series_episodes_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Проверяем, что это сериал
    let media_item = state
        .db
        .get_media_item_by_id(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if media_item.media_type != MediaType::TvSeries {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Получаем эпизоды
    let episodes = state
        .db
        .get_episodes(&id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Группируем по сезонам
    use std::collections::HashMap;
    let mut seasons: HashMap<u32, Vec<_>> = HashMap::new();
    for episode in episodes {
        seasons
            .entry(episode.season_number)
            .or_insert_with(Vec::new)
            .push(episode);
    }

    // Обложки сезонов ключуются номером сезона — тем же, что и группы эпизодов
    let posters: HashMap<String, String> = state
        .db
        .get_season_posters(&id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(season, url)| (season.to_string(), url))
        .collect();

    Ok(Json(serde_json::json!({
        "series_id": id,
        "seasons": seasons,
        "season_posters": posters
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hevc_needs_video_transcoding() {
        assert!(needs_video_transcoding(
            &Some("hevc".to_string()),
            &Some("yuv420p".to_string()),
            &None
        ));
    }

    #[test]
    fn vp9_needs_video_transcoding() {
        assert!(needs_video_transcoding(
            &Some("vp9".to_string()),
            &Some("yuv420p".to_string()),
            &None
        ));
    }

    #[test]
    fn ten_bit_needs_video_transcoding() {
        assert!(needs_video_transcoding(
            &Some("h264".to_string()),
            &Some("yuv420p10le".to_string()),
            &None
        ));
    }

    #[test]
    fn h264_8bit_does_not_need_video_transcoding() {
        assert!(!needs_video_transcoding(
            &Some("h264".to_string()),
            &Some("yuv420p".to_string()),
            &None
        ));
    }

    #[test]
    fn unknown_codec_and_pix_fmt_does_not_need_video_transcoding() {
        assert!(!needs_video_transcoding(&None, &None, &None));
    }

    #[test]
    fn mpeg4_needs_video_transcoding() {
        assert!(needs_video_transcoding(
            &Some("mpeg4".to_string()),
            &Some("yuv420p".to_string()),
            &None
        ));
    }

    /// Регрессия: запрос init.mp4 раньше запускал полноразмерный ран с нулевого сегмента.
    /// При старте с середины файла это означало параллельный перекод всего фильма впустую
    /// (в логе: ран с 0 дошёл до сегмента 205, пока смотрели со 104-го).
    #[tokio::test]
    async fn init_does_not_start_full_file_run() {
        let tmp = tempfile::tempdir().unwrap();
        // mpeg4 → перекодирование видео, то есть fMP4 и init.mp4. Форма из матрицы,
        // а не самодельная фикстура.
        let shape = crate::testkit::Shape::matrix()
            .into_iter()
            .find(|s| s.name == "dvdrip_mpeg4_aac")
            .expect("форма dvdrip_mpeg4_aac пропала из матрицы");
        let file = shape.build(tmp.path());
        let (state, id) = state_for_shape(&file, &shape).await;

        let (status, _, _) = get(&state, &format!("/api/media/{id}/hls/init.mp4"), None).await;
        assert_eq!(status, StatusCode::OK, "init.mp4");

        let cache_dir = std::fs::read_dir(state.hls.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .expect("каталог кэша не создан");

        let names: Vec<String> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert!(
            names.iter().any(|n| n == "init.mp4"),
            "нет init.mp4: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("segment")),
            "init-ран оставил сегменты в кэше: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "init-scratch"),
            "рабочий каталог init-рана не убран: {names:?}"
        );
    }

    /// Устаревший VOD-манифест (hls.js их не перезапрашивает) должен получать 404,
    /// а не бесконечные ретраи по мёртвым URL.
    #[tokio::test]
    async fn stale_playlist_marker_gets_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_ac3.mkv", "ac3");
        let (state, id) = state_with_movie(&file, "ac3").await;

        // Профиль берём из самого плейлиста: он зависит не только от кодеков (короткая
        // фикстура откатывается на перекодирование из-за нехватки keyframes).
        let (_, _, body) = get(&state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        let playlist = String::from_utf8_lossy(&body).to_string();
        let uri = playlist
            .lines()
            .find(|l| l.starts_with("segment"))
            .expect("в плейлисте нет сегментов")
            .to_string();
        let (name, query) = uri.split_once("?p=").expect("в URI нет маркера профиля");
        let (current, _audio) = query.split_once("&a=").expect("в URI нет маркера дорожки");

        let (status, _, _) = get(&state, &format!("/api/media/{id}/hls/{uri}"), None).await;
        assert_eq!(status, StatusCode::OK, "сегмент текущего профиля {uri}");

        // Тот же сегмент с маркером другого профиля — устаревший плейлист.
        let stale = if current == "av" { "a" } else { "av" };
        let (status, _, _) = get(
            &state,
            &format!("/api/media/{id}/hls/{name}?p={stale}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "сегмент чужого профиля");
    }

    /// Регрессия: файлы, попавшие в БД до появления признака, остаются с NULL навсегда —
    /// `insert_file_if_new` пропускает известные файлы, поэтому ре-скан их не чинит.
    #[tokio::test]
    async fn backfill_fills_field_order_for_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let shape = crate::testkit::Shape {
            name: "backfill_interlaced",
            video: crate::testkit::VideoForm::H264Interlaced,
            audio: crate::testkit::AudioForm::Mp2,
            timestamps: crate::testkit::TimestampForm::Clean,
            gop: crate::testkit::GopForm::Uniform,
            seconds: 8,
            extra_audio: &[],
        };
        let file = shape.build(tmp.path());

        // Так выглядит запись, сделанная до миграции 008.
        let (state, _) = state_with_field_order(&file, "h264", "mp2", 8, None).await;

        let filled = backfill_field_order(&state).await;
        assert_eq!(filled, 1, "бэкфилл должен был обработать один файл");

        let order: Option<String> =
            sqlx::query_scalar("SELECT field_order FROM media_files LIMIT 1")
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
        assert_eq!(order.as_deref(), Some("tt"));

        // Второй проход не должен находить работу — иначе ffprobe гонялся бы каждый старт.
        assert_eq!(backfill_field_order(&state).await, 0);
    }

    /// Регрессия: без бэкфилла у собранной до миграции 009 библиотеки список дорожек
    /// остался бы пустым, и плеер не показал бы выбор перевода ни у одного фильма.
    #[tokio::test]
    async fn backfill_fills_audio_tracks_for_existing_files() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let shape = crate::testkit::Shape::matrix()
            .into_iter()
            .find(|s| !s.extra_audio.is_empty())
            .expect("в матрице нет многодорожечной формы");
        let tmp = tempfile::tempdir().unwrap();
        let file = shape.build(tmp.path());
        let (state, _) = state_for_shape(&file, &shape).await;

        // Так выглядит запись, сделанная до миграции 009.
        sqlx::query("UPDATE media_files SET audio_tracks = NULL")
            .execute(&state.db.pool)
            .await
            .unwrap();

        assert_eq!(backfill_audio_tracks(&state).await, 1);

        let file = state
            .db
            .resolve_media_file(&state.db.get_media_items(None).await.unwrap()[0].id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            file.audio_tracks.len(),
            shape.extra_audio.len() + 1,
            "бэкфилл должен прочитать все дорожки: {:?}",
            file.audio_tracks
        );

        // Второй проход не должен находить работу — иначе ffprobe гонялся бы каждый старт.
        assert_eq!(backfill_audio_tracks(&state).await, 0);
    }

    /// Чересстрочный H.264 проходит белый список кодеков и без этой проверки уходил бы
    /// в `-c:v copy` — MSE поля не декодирует, отсюда рывки при копеечном битрейте.
    #[test]
    fn interlaced_h264_needs_video_transcoding() {
        for order in ["tt", "bb", "tb", "bt"] {
            assert!(
                needs_video_transcoding(
                    &Some("h264".to_string()),
                    &Some("yuv420p".to_string()),
                    &Some(order.to_string())
                ),
                "field_order={order} должен требовать перекодирования"
            );
        }
    }

    /// "unknown" и отсутствие метаданных — не повод жечь CPU на всей библиотеке.
    #[test]
    fn progressive_and_unknown_field_order_stay_in_copy() {
        for order in [None, Some("progressive"), Some("unknown")] {
            assert!(!needs_video_transcoding(
                &Some("h264".to_string()),
                &Some("yuv420p".to_string()),
                &order.map(|o| o.to_string())
            ));
        }
    }

    #[test]
    fn mp3_needs_audio_transcoding() {
        assert!(needs_audio_transcoding(&Some("mp3".to_string())));
    }

    #[test]
    fn ac3_needs_audio_transcoding() {
        assert!(needs_audio_transcoding(&Some("ac3".to_string())));
    }

    #[test]
    fn aac_does_not_need_audio_transcoding() {
        assert!(!needs_audio_transcoding(&Some("aac".to_string())));
    }

    #[test]
    fn unknown_audio_codec_does_not_need_transcoding() {
        assert!(!needs_audio_transcoding(&None));
    }

    // --- Слой 1a: parse_range ---

    #[test]
    fn parse_range_open_ended() {
        assert_eq!(parse_range("bytes=0-", 1000), ByteRange::Part(0, 999));
    }

    #[test]
    fn parse_range_explicit() {
        assert_eq!(parse_range("bytes=100-200", 1000), ByteRange::Part(100, 200));
    }

    #[test]
    fn parse_range_from_middle_to_end() {
        assert_eq!(parse_range("bytes=500-", 1000), ByteRange::Part(500, 999));
    }

    #[test]
    fn parse_range_suffix_is_last_bytes() {
        assert_eq!(parse_range("bytes=-200", 1000), ByteRange::Part(800, 999));
        assert_eq!(parse_range("bytes=-2000", 1000), ByteRange::Part(0, 999));
        assert_eq!(parse_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
    }

    #[test]
    fn parse_range_start_after_end_is_ignored() {
        assert_eq!(parse_range("bytes=800-200", 1000), ByteRange::Whole);
    }

    #[test]
    fn parse_range_end_beyond_size_is_clamped() {
        assert_eq!(parse_range("bytes=0-2000", 1000), ByteRange::Part(0, 999));
    }

    #[test]
    fn parse_range_start_beyond_size_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=1000-", 1000), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=-100", 0), ByteRange::Unsatisfiable);
    }

    #[test]
    fn parse_range_garbage_is_ignored() {
        assert_eq!(parse_range("bytes=abc", 1000), ByteRange::Whole);
        assert_eq!(parse_range("100-200", 1000), ByteRange::Whole); // нет префикса bytes=
        assert_eq!(parse_range("bytes=1-2-3", 1000), ByteRange::Whole);
    }

    // --- Слой 1b: range_start_seconds ---

    #[test]
    fn range_start_seconds_middle_with_known_duration() {
        // Половина файла при длительности 100 сек → ~50 сек.
        assert_eq!(range_start_seconds(500, 1000, Some(100)), 50);
    }

    #[test]
    fn range_start_seconds_from_zero() {
        assert_eq!(range_start_seconds(0, 1000, Some(100)), 0);
    }

    #[test]
    fn range_start_seconds_estimates_duration_when_unknown() {
        // file_size=125_000_000 байт при ~5 Мбит/с → ~200 сек; половина → ~100 сек.
        let secs = range_start_seconds(62_500_000, 125_000_000, None);
        assert!((95..=105).contains(&secs), "ожидали ~100, получили {secs}");
    }

    #[test]
    fn range_start_seconds_zero_file_size_does_not_panic() {
        // Регрессия: деление на ноль при file_size=0 не должно паниковать.
        assert_eq!(range_start_seconds(0, 0, Some(100)), 0);
        assert_eq!(range_start_seconds(0, 0, None), 0);
    }

    // --- Слой 2: интеграционные тесты стриминга/промотки (нужен ffmpeg) ---

    use crate::config::Config;
    use crate::metrics::MetricsCollector;
    use crate::models::MediaFile;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt; // oneshot

    /// true, если ffmpeg и ffprobe доступны в PATH.
    fn ffmpeg_available() -> bool {
        let ok = |bin: &str| {
            std::process::Command::new(bin)
                .arg("-version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        ok("ffmpeg") && ok("ffprobe")
    }

    /// Генерирует крошечный тестовый файл (8 сек) с заданным аудиокодеком.
    fn make_fixture(dir: &std::path::Path, name: &str, audio_codec: &str) -> PathBuf {
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=8:size=320x240:rate=25",
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=8"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .args(["-c:a", audio_codec])
            .arg("-shortest")
            .arg(&path)
            .status()
            .expect("не удалось запустить ffmpeg для фикстуры");
        assert!(status.success(), "ffmpeg не сгенерировал фикстуру {name}");
        path
    }

    /// Фикстура с *неравномерными* keyframes: карта сегментов строится по реальному индексу,
    /// и любая попытка угадать резы по равномерной сетке немедленно с ней разъезжается.
    fn make_fixture_with_keyframes(
        dir: &std::path::Path,
        name: &str,
        audio_codec: &str,
        seconds: u32,
        keyframes: &str,
    ) -> PathBuf {
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .args([
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc=duration={seconds}:size=320x240:rate=25"),
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:duration={seconds}"),
            ])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .args(["-sc_threshold", "0", "-force_key_frames", keyframes])
            .args(["-c:a", audio_codec])
            .arg("-shortest")
            .arg(&path)
            .status()
            .expect("не удалось запустить ffmpeg для фикстуры");
        assert!(status.success(), "ffmpeg не сгенерировал фикстуру {name}");
        path
    }

    /// Собирает AppState с in-memory БД и одним фильмом, ссылающимся на `file_path`.
    /// Возвращает (state, media_id фильма).
    async fn state_with_movie(
        file_path: &std::path::Path,
        audio_codec: &str,
    ) -> (AppState, String) {
        state_with_movie_duration(file_path, audio_codec, 8).await
    }

    async fn state_with_movie_duration(
        file_path: &std::path::Path,
        audio_codec: &str,
        duration: u64,
    ) -> (AppState, String) {
        state_with_codecs(file_path, "h264", audio_codec, duration).await
    }

    async fn state_with_codecs(
        file_path: &std::path::Path,
        video_codec: &str,
        audio_codec: &str,
        duration: u64,
    ) -> (AppState, String) {
        state_with_field_order(file_path, video_codec, audio_codec, duration, None).await
    }

    async fn state_with_field_order(
        file_path: &std::path::Path,
        video_codec: &str,
        audio_codec: &str,
        duration: u64,
        field_order: Option<&str>,
    ) -> (AppState, String) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/001_initial_schema.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/002_add_pix_fmt.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/003_tmdb_metadata.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/004_segment_index.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/006_custom_title.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/007_season_posters.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/008_add_field_order.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../migrations/009_audio_tracks.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let db = crate::database::Database { pool };

        let file_id = db
            .insert_media_file(&MediaFile {
                id: 0,
                file_path: file_path.to_path_buf(),
                duration: Some(duration),
                resolution: None,
                video_codec: Some(video_codec.into()),
                audio_codec: Some(audio_codec.into()),
                video_bitrate: None,
                pix_fmt: Some("yuv420p".into()),
                field_order: field_order.map(|f| f.to_string()),
                // Дорожки берём из самого файла: у многодорожечных фикстур тест
                // проверяет выбор конкретной дорожки, а не переданный кодек.
                audio_tracks: crate::metadata::extract_metadata(file_path)
                    .map(|m| m.audio_tracks)
                    .unwrap_or_default(),
            })
            .await
            .unwrap();
        let movie_id = db
            .insert_media_item(None, "Тест", None, &MediaType::Movie, Some(file_id))
            .await
            .unwrap();

        let config = Config {
            port: 0,
            media_paths: vec![],
            log_level: "info".into(),
            database_url: "sqlite::memory:".into(),
            tmdb_api_key: None,
            tmdb_language: "ru-RU".into(),
            cache_dir: PathBuf::from("./cache"),
            strict_auth: true,
            ffmpeg_max_runs: crate::transcoder::DEFAULT_MAX_RUNS,
            jwt_secret: "test-secret".into(),
        };
        let state = AppState {
            config: Arc::new(config),
            db,
            metrics: Arc::new(MetricsCollector::new()),
            // Свой корень кэша на каждый AppState: иначе прогон подхватывает сегменты,
            // нарезанные предыдущей версией кода, и зелёный тест ничего не значит.
            hls: HlsManager::with_root(unique_cache_root()),
            scan_jobs: Arc::new(RwLock::new(HashMap::new())),
            pairing: crate::pairing::PairingManager::new(),
            tmdb: None,
        };
        (state, movie_id)
    }

    fn unique_cache_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aurora-test-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn get(
        state: &AppState,
        uri: &str,
        range: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let token = create_token(1, "test", &state.config).unwrap();
        let mut builder = Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(range) = range {
            builder = builder.header(header::RANGE, range);
        }
        let req = builder.body(Body::empty()).unwrap();
        let resp = create_router(state.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    /// Сценарий 1: прямой стрим с Range (промотка вперёд по байтам).
    #[tokio::test]
    async fn direct_stream_range_returns_partial_content() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_aac.mp4", "aac");
        let (state, id) = state_with_movie(&file, "aac").await;

        let (status, headers, body) = get(
            &state,
            &format!("/api/media/{id}/stream"),
            Some("bytes=1000-2000"),
        )
        .await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            headers
                .get(header::CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            &format!(
                "bytes 1000-2000/{}",
                std::fs::metadata(&file).unwrap().len()
            )
        );
        assert_eq!(body.len(), 1001);
    }

    /// Сценарий 2: прямой стрим без Range → весь файл + Accept-Ranges.
    #[tokio::test]
    async fn direct_stream_without_range_returns_full_file() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_aac.mp4", "aac");
        let (state, id) = state_with_movie(&file, "aac").await;

        let (status, headers, body) = get(&state, &format!("/api/media/{id}/stream"), None).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");
        assert_eq!(body.len() as u64, std::fs::metadata(&file).unwrap().len());
    }

    /// Сценарий 6: transcoded на aac-файле делегирует в обычный стрим (транскод НЕ включается).
    #[tokio::test]
    async fn transcoded_delegates_for_browser_codec() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_aac.mp4", "aac");
        let (state, id) = state_with_movie(&file, "aac").await;

        let (status, headers, _) = get(
            &state,
            &format!("/api/media/{id}/stream-transcoded"),
            Some("bytes=0-1000"),
        )
        .await;

        // Делегация в stream_handler → Range обработан как обычный файл (206 + Content-Range).
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert!(headers.get(header::CONTENT_RANGE).is_some());
    }

    /// Сценарий 3+4: transcoded на ac3 отдаёт валидный fMP4, промотка через ?start=.
    #[tokio::test]
    async fn transcoded_ac3_produces_valid_mp4_and_seeks() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_ac3.mkv", "ac3");
        let (state, id) = state_with_movie(&file, "ac3").await;

        for start in [0u64, 3u64] {
            let (status, headers, body) = get(
                &state,
                &format!("/api/media/{id}/stream-transcoded?start={start}"),
                None,
            )
            .await;

            assert_eq!(status, StatusCode::OK, "start={start}");
            assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "video/mp4");
            assert!(!body.is_empty(), "пустое тело при start={start}");
            // Валидный фрагментированный MP4 начинается с box 'ftyp'.
            assert!(
                body.windows(4).take(64).any(|w| w == b"ftyp"),
                "нет ftyp-сигнатуры MP4 при start={start}"
            );
        }
    }

    /// Плейлист — VOD на весь файл: PLAYLIST-TYPE, ENDLIST, MAP и хотя бы один сегмент сразу,
    /// без ожидания генерации.
    #[tokio::test]
    async fn hls_playlist_is_vod_and_complete() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_ac3.mkv", "ac3");
        let (state, id) = state_with_movie(&file, "ac3").await;

        let (status, headers, body) =
            get(&state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/vnd.apple.mpegurl"
        );
        let playlist = String::from_utf8_lossy(&body).to_string();
        assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(playlist.contains("#EXT-X-ENDLIST"));
        assert!(playlist.contains("#EXT-X-MAP"));
        assert!(playlist.contains("#EXTINF:"));
    }

    /// Карта сегментов делает генерацию ленивой и независимой от порядка: запрос сегмента
    /// из середины первым (до всякого обращения к нулевому) обязан отработать без 404.
    #[tokio::test]
    async fn hls_segment_from_middle_served_first() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture(tmp.path(), "movie_ac3.mkv", "ac3");
        let (state, id) = state_with_movie(&file, "ac3").await;

        let (_, _, playlist_body) =
            get(&state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        let playlist = String::from_utf8_lossy(&playlist_body).to_string();
        let segments: Vec<&str> = playlist
            .lines()
            .filter(|line| line.contains(".m4s"))
            .collect();
        let middle = segments[segments.len() / 2];

        let (status, _, body) = get(&state, &format!("/api/media/{id}/hls/{middle}"), None).await;

        assert_eq!(status, StatusCode::OK, "сегмент {middle} из середины");
        assert!(!body.is_empty());
    }

    /// То же для рана, стартующего с середины (перемотка): ffmpeg запускается с `-ss` и своим
    /// списком точек реза, но границы обязаны остаться теми же, что в плейлисте.
    #[tokio::test]
    async fn copy_profile_middle_segment_matches_playlist() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture_with_keyframes(
            tmp.path(),
            "movie_ac3.mkv",
            "ac3",
            30,
            "0,4,9.5,14.5,17,19.5,22,25.5",
        );
        let (state, id) = state_with_movie_duration(&file, "ac3", 30).await;

        let (_, _, playlist_body) =
            get(&state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        let playlist = String::from_utf8_lossy(&playlist_body).to_string();
        let lines: Vec<&str> = playlist.lines().collect();
        // Берём предпоследний сегмент — до него никто не обращался, ран пойдёт с середины
        let pos = lines.iter().rposition(|l| l.contains(".ts")).unwrap() - 2;
        let name = lines[pos];
        let want: f64 = lines[pos - 1]
            .strip_prefix("#EXTINF:")
            .unwrap()
            .trim_end_matches(',')
            .parse()
            .unwrap();

        let (status, _, body) = get(&state, &format!("/api/media/{id}/hls/{name}"), None).await;
        assert_eq!(status, StatusCode::OK, "сегмент {name}");

        let seg_path = tmp.path().join("middle.ts");
        std::fs::write(&seg_path, &body).unwrap();
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration"])
            .args(["-of", "csv=p=0"])
            .arg(&seg_path)
            .output()
            .unwrap();
        let got: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();

        assert!(
            (got - want).abs() < 0.5,
            "сегмент {name} из середины: в плейлисте {want:.3} c, в данных {got:.3} c"
        );
    }

    /// Главная гарантия copy-профиля: сегмент под номером N содержит ровно тот отрезок, который
    /// обещан в плейлисте. Если резы муксера разъедутся с картой, плеер начнёт то и дело
    /// подвисать, доклеивая буфер не оттуда.
    #[tokio::test]
    async fn copy_profile_segments_match_playlist_durations() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = make_fixture_with_keyframes(
            tmp.path(),
            "movie_ac3.mkv",
            "ac3",
            30,
            "0,4,9.5,14.5,17,19.5,22,25.5",
        );
        let (state, id) = state_with_movie_duration(&file, "ac3", 30).await;

        let (_, _, playlist_body) =
            get(&state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        let playlist = String::from_utf8_lossy(&playlist_body).to_string();

        // Видео h264 копируется — контейнер TS, init-сегмента нет
        assert!(!playlist.contains("#EXT-X-MAP"), "{playlist}");

        let mut expected: Vec<(String, f64)> = Vec::new();
        let mut lines = playlist.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(dur) = line.strip_prefix("#EXTINF:") {
                let dur: f64 = dur.trim_end_matches(',').parse().unwrap();
                let name = lines.next().unwrap().to_string();
                assert!(name.contains(".ts"), "неожиданное имя сегмента {name}");
                expected.push((name, dur));
            }
        }
        assert!(expected.len() > 2, "мало сегментов: {}", expected.len());

        for (name, want) in &expected {
            let (status, _, body) = get(&state, &format!("/api/media/{id}/hls/{name}"), None).await;
            assert_eq!(status, StatusCode::OK, "сегмент {name}");

            let seg_path = tmp.path().join(name);
            std::fs::write(&seg_path, &body).unwrap();
            let out = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "format=duration"])
                .args(["-of", "csv=p=0"])
                .arg(&seg_path)
                .output()
                .unwrap();
            let got: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();

            assert!(
                (got - want).abs() < 0.5,
                "сегмент {name}: в плейлисте {want:.3} c, в данных {got:.3} c"
            );
        }
    }

    // --- Слой 3: матрица форм источника ---

    /// Как слой 2, но кодеки берутся из формы, а не захардкожены: от них зависит профиль,
    /// а значит и вся ветка нарезки.
    async fn state_for_shape(
        file_path: &std::path::Path,
        shape: &crate::testkit::Shape,
    ) -> (AppState, String) {
        let video_codec = match shape.video {
            crate::testkit::VideoForm::H264 | crate::testkit::VideoForm::H264Interlaced => "h264",
            crate::testkit::VideoForm::Mpeg4 => "mpeg4",
        };
        // field_order берём из самого файла, а не из формы: так тест ловит и случай,
        // когда сканер перестал класть признак в БД.
        let field_order = crate::testkit::probe_field_order(file_path);
        state_with_field_order(
            file_path,
            video_codec,
            shape.audio.probed_name(),
            shape.seconds as u64,
            Some(field_order.as_str()).filter(|f| !f.is_empty()),
        )
        .await
    }

    /// Скачивает плейлист и все его сегменты на диск в том виде, в каком их получит плеер.
    /// Для fMP4 к каждому сегменту приклеивается init — без него `.m4s` не разбирается,
    /// как и в браузере.
    async fn fetch_segments(
        state: &AppState,
        id: &str,
        dir: &std::path::Path,
    ) -> Vec<crate::testkit::SegmentUnderTest> {
        let (status, _, body) =
            get(state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        assert_eq!(status, StatusCode::OK, "плейлист");
        let playlist = String::from_utf8_lossy(&body).to_string();

        let init = if playlist.contains("#EXT-X-MAP") {
            let (status, _, body) =
                get(state, &format!("/api/media/{id}/hls/init.mp4"), None).await;
            assert_eq!(status, StatusCode::OK, "init.mp4");
            Some(body)
        } else {
            None
        };

        let mut out = Vec::new();
        let mut lines = playlist.lines().peekable();
        while let Some(line) = lines.next() {
            let Some(dur) = line.strip_prefix("#EXTINF:") else {
                continue;
            };
            let extinf: f64 = dur.trim_end_matches(',').parse().unwrap();
            let name = lines.next().unwrap().to_string();

            let (status, _, body) = get(state, &format!("/api/media/{id}/hls/{name}"), None).await;
            assert_eq!(status, StatusCode::OK, "сегмент {name}");

            let path = dir.join(&name);
            let mut bytes = init.clone().unwrap_or_default();
            bytes.extend_from_slice(&body);
            std::fs::write(&path, &bytes).unwrap();

            out.push(crate::testkit::SegmentUnderTest { name, path, extinf });
        }
        out
    }

    /// Один и тот же чеклист проигрываемости по всем формам источника.
    ///
    /// Смысл теста — снять зависимость от контента: «другой сериал» перестаёт быть новой
    /// сущностью, он либо попадает в известную клетку матрицы, либо матрицу надо расширить.
    /// Оба дорогих бага (рассинхрон карты и потеря видео в нулевом сегменте) видны здесь и
    /// невидимы на «удобной» фикстуре `clean_h264_aac`, которая лежит в матрице как
    /// контрольная точка.
    #[tokio::test]
    async fn every_source_shape_produces_playable_segments() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }

        for shape in crate::testkit::Shape::matrix() {
            let tmp = tempfile::tempdir().unwrap();
            let file = shape.build(tmp.path());
            let (state, id) = state_for_shape(&file, &shape).await;

            let out = tmp.path().join("out");
            std::fs::create_dir_all(&out).unwrap();
            let segments = fetch_segments(&state, &id, &out).await;

            crate::testkit::assert_playable(shape.name, &segments);
        }
    }

    /// Тон дорожки в герцах, посчитанный по переходам через ноль. Кодек сегмента ничего
    /// не говорит о том, какую дорожку взял ffmpeg, а тон переживает и перекодирование.
    fn dominant_tone_hz(path: &std::path::Path) -> f64 {
        const RATE: usize = 8000;
        let out = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-map", "0:a:0", "-ac", "1", "-ar", "8000"])
            .args(["-f", "s16le", "-"])
            .output()
            .unwrap();
        let samples: Vec<i16> = out
            .stdout
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert!(samples.len() > RATE / 2, "в сегменте почти нет звука");
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        crossings as f64 / 2.0 / (samples.len() as f64 / RATE as f64)
    }

    /// Выбор дорожки: релиз с несколькими переводами должен отдавать именно запрошенную.
    /// Фикстура кладёт в каждую дорожку свой тон (440/660/880 Гц) — по нему и проверяем.
    #[tokio::test]
    async fn requested_audio_track_is_the_one_served() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let shape = crate::testkit::Shape::matrix()
            .into_iter()
            .find(|s| !s.extra_audio.is_empty())
            .expect("в матрице нет многодорожечной формы");
        let tmp = tempfile::tempdir().unwrap();
        let file = shape.build(tmp.path());
        let (state, id) = state_for_shape(&file, &shape).await;

        for (track, want_hz) in [(0u32, 440.0), (1, 660.0), (2, 880.0)] {
            let (status, _, body) = get(
                &state,
                &format!("/api/media/{id}/hls/playlist.m3u8?audio={track}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "плейлист дорожки {track}");
            let playlist = String::from_utf8_lossy(&body).to_string();
            let name = playlist
                .lines()
                .find(|l| l.starts_with("segment"))
                .expect("в плейлисте нет сегментов")
                .to_string();
            assert!(
                name.contains(&format!("&a={track}")),
                "URI сегмента без маркера дорожки: {name}"
            );

            let mut bytes = if playlist.contains("#EXT-X-MAP") {
                let uri = playlist
                    .lines()
                    .find_map(|l| l.strip_prefix("#EXT-X-MAP:URI=\""))
                    .and_then(|l| l.split('"').next())
                    .unwrap()
                    .to_string();
                let (status, _, body) =
                    get(&state, &format!("/api/media/{id}/hls/{uri}"), None).await;
                assert_eq!(status, StatusCode::OK, "init.mp4 дорожки {track}");
                body
            } else {
                Vec::new()
            };

            let (status, _, body) = get(&state, &format!("/api/media/{id}/hls/{name}"), None).await;
            assert_eq!(status, StatusCode::OK, "сегмент дорожки {track}");
            bytes.extend_from_slice(&body);

            let path = tmp.path().join(format!("track{track}.seg"));
            std::fs::write(&path, &bytes).unwrap();
            let got = dominant_tone_hz(&path);
            assert!(
                (got - want_hz).abs() < 60.0,
                "дорожка {track}: ожидали ~{want_hz} Гц, получили {got:.0} Гц"
            );
        }

        // Сегменты разных дорожек не должны делить каталог кэша.
        let dirs = std::fs::read_dir(state.hls.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("aurora-hls-"))
            .count();
        assert_eq!(
            dirs, 3,
            "у каждой дорожки должен быть свой каталог сегментов"
        );
    }

    /// Прогон по реальной библиотеке: по одному файлу на каждую группу
    /// `(video_codec, audio_codec, pix_fmt)`.
    ///
    /// Роль теста — не покрытие, а разведка: он ищет форму источника, которой нет в
    /// [`crate::testkit::Shape::matrix`]. Нашлась — ось добавляется в матрицу, и дальше
    /// держится синтетикой, без зависимости от того, что лежит на диске.
    ///
    /// Не входит в обычный прогон (нужна настоящая библиотека):
    /// `cargo test real_library_conformance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "нужна реальная библиотека; запускать вручную"]
    async fn real_library_conformance() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg/ffprobe недоступны");
            return;
        }
        let db_path = std::env::var("AURORA_DB").unwrap_or_else(|_| "data/aurora.db".into());
        if !std::path::Path::new(&db_path).exists() {
            eprintln!("SKIP: нет {db_path}");
            return;
        }

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{db_path}?mode=ro"))
            .await
            .unwrap();

        // По одному представителю каждой группы: разные файлы одной группы идут по одному
        // и тому же пути через код.
        /// (file_path, video_codec, audio_codec, pix_fmt, duration)
        type LibraryRow = (String, Option<String>, Option<String>, Option<String>, i64);
        let rows: Vec<LibraryRow> = sqlx::query_as(
            "select file_path, video_codec, audio_codec, pix_fmt, coalesce(duration, 0)
                 from media_files
                 where duration > 60
                 group by video_codec, audio_codec, pix_fmt",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!rows.is_empty(), "в библиотеке нет файлов с длительностью");

        for (file_path, video_codec, audio_codec, pix_fmt, duration) in rows {
            let path = PathBuf::from(&file_path);
            if !path.exists() {
                eprintln!("пропуск (файла нет на диске): {file_path}");
                continue;
            }
            let group = format!(
                "{}/{}/{}",
                video_codec.clone().unwrap_or_default(),
                audio_codec.clone().unwrap_or_default(),
                pix_fmt.clone().unwrap_or_default()
            );
            eprintln!("--- {group}: {file_path}");

            let (state, id) = state_with_codecs(
                &path,
                video_codec.as_deref().unwrap_or("h264"),
                audio_codec.as_deref().unwrap_or("aac"),
                duration as u64,
            )
            .await;

            let tmp = tempfile::tempdir().unwrap();
            // Целиком гонять полуторачасовую серию бессмысленно: берём непрерывные окна с
            // начала, середины и конца — стык проверяется внутри окна.
            let all = fetch_playlist_entries(&state, &id).await;
            let windows = [0usize, all.len() / 2, all.len().saturating_sub(4)];
            for start in windows {
                let take: Vec<_> = all.iter().skip(start).take(4).cloned().collect();
                if take.len() < 3 {
                    continue;
                }
                let segments = fetch_named_segments(&state, &id, tmp.path(), &take).await;
                crate::testkit::assert_playable(&format!("{group}@{start}"), &segments);
            }
        }
    }

    /// Пары (имя сегмента, EXTINF) из плейлиста.
    async fn fetch_playlist_entries(state: &AppState, id: &str) -> Vec<(String, f64)> {
        let (status, _, body) =
            get(state, &format!("/api/media/{id}/hls/playlist.m3u8"), None).await;
        assert_eq!(status, StatusCode::OK, "плейлист");
        let playlist = String::from_utf8_lossy(&body).to_string();

        let mut out = Vec::new();
        let mut lines = playlist.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(dur) = line.strip_prefix("#EXTINF:") {
                let extinf: f64 = dur.trim_end_matches(',').parse().unwrap();
                out.push((lines.next().unwrap().to_string(), extinf));
            }
        }
        out
    }

    async fn fetch_named_segments(
        state: &AppState,
        id: &str,
        dir: &std::path::Path,
        entries: &[(String, f64)],
    ) -> Vec<crate::testkit::SegmentUnderTest> {
        // Имя сегмента в плейлисте несёт маркеры (`?p=…&a=…`), поэтому проверяем вхождение
        // расширения, а не конец строки: иначе init не подклеивается и fMP4 не читается.
        let init = if entries.iter().any(|(n, _)| n.contains(".m4s")) {
            let (status, _, body) =
                get(state, &format!("/api/media/{id}/hls/init.mp4"), None).await;
            assert_eq!(status, StatusCode::OK, "init.mp4");
            Some(body)
        } else {
            None
        };

        let mut out = Vec::new();
        for (name, extinf) in entries {
            let (status, _, body) = get(state, &format!("/api/media/{id}/hls/{name}"), None).await;
            assert_eq!(status, StatusCode::OK, "сегмент {name}");
            let path = dir.join(name);
            let mut bytes = init.clone().unwrap_or_default();
            bytes.extend_from_slice(&body);
            std::fs::write(&path, &bytes).unwrap();
            out.push(crate::testkit::SegmentUnderTest {
                name: name.clone(),
                path,
                extinf: *extinf,
            });
        }
        out
    }

    /// Запрос к защищённому эндпоинту с заданным адресом клиента и токеном.
    /// Адрес кладётся как `ConnectInfo` — так же, как его проставляет `axum::serve`.
    async fn auth_status(state: &AppState, peer: &str, token: Option<&str>) -> StatusCode {
        let mut builder = Request::builder().uri("/api/libraries");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let mut req = builder.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        create_router(state.clone())
            .oneshot(req)
            .await
            .unwrap()
            .status()
    }

    async fn auth_state(strict_auth: bool) -> AppState {
        let (mut state, _) =
            state_with_movie(std::path::Path::new("/nonexistent.mp4"), "aac").await;
        state.db.create_user("home", "hash").await.unwrap();
        let mut config = (*state.config).clone();
        config.strict_auth = strict_auth;
        state.config = Arc::new(config);
        state
    }

    #[tokio::test]
    async fn lan_client_needs_no_token_by_default() {
        let state = auth_state(false).await;
        assert_eq!(
            auth_status(&state, "192.168.1.20:5000", None).await,
            StatusCode::OK
        );
        assert_eq!(
            auth_status(&state, "127.0.0.1:5000", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn proxied_lan_client_needs_no_token() {
        let state = auth_state(false).await;
        for name in ["x-forwarded-for", "x-real-ip", "forwarded"] {
            let mut req = Request::builder()
                .uri("/api/libraries")
                .header(name, "8.8.8.8")
                .body(Body::empty())
                .unwrap();
            req.extensions_mut()
                .insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
            let status = create_router(state.clone())
                .oneshot(req)
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::OK, "{name}");
        }
    }

    #[tokio::test]
    async fn outside_client_needs_valid_token() {
        let state = auth_state(false).await;
        let token = create_token(1, "home", &state.config).unwrap();
        assert_eq!(
            auth_status(&state, "8.8.8.8:5000", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            auth_status(&state, "8.8.8.8:5000", Some("garbage")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            auth_status(&state, "8.8.8.8:5000", Some(&token)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn strict_auth_requires_token_from_lan() {
        let state = auth_state(true).await;
        assert_eq!(
            auth_status(&state, "192.168.1.20:5000", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// Запрос без токена из LAN с заданным `Origin` — так его шлёт браузер
    async fn lan_status_with_origin(
        state: &AppState,
        method: &str,
        uri: &str,
        origin: &str,
    ) -> StatusCode {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "192.168.1.5:4000")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(
            "192.168.1.20:5000".parse::<SocketAddr>().unwrap(),
        ));
        create_router(state.clone())
            .oneshot(req)
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn same_origin_lan_page_needs_no_token() {
        let state = auth_state(false).await;
        assert_eq!(
            lan_status_with_origin(&state, "GET", "/api/libraries", "http://192.168.1.5:4000")
                .await,
            StatusCode::OK
        );
        assert_eq!(
            lan_status_with_origin(&state, "POST", "/api/auth/lan", "http://192.168.1.5:4000")
                .await,
            StatusCode::OK
        );
    }

    /// Чужой сайт, открытый в домашнем браузере, не должен получить доступ через LAN-доверие
    #[tokio::test]
    async fn cross_origin_page_gets_no_lan_trust() {
        let state = auth_state(false).await;
        for origin in ["https://evil.example", "null"] {
            assert_eq!(
                lan_status_with_origin(&state, "GET", "/api/libraries", origin).await,
                StatusCode::UNAUTHORIZED,
                "{origin}"
            );
            assert_eq!(
                lan_status_with_origin(&state, "POST", "/api/auth/lan", origin).await,
                StatusCode::FORBIDDEN,
                "{origin}"
            );
        }
    }

    /// Клиент продлевает токен при запуске — пока устройством пользуются, вход не нужен
    #[tokio::test]
    async fn refresh_issues_a_fresh_long_token() {
        let state = auth_state(false).await;
        let token = create_token(1, "home", &state.config).unwrap();
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/auth/refresh")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo("8.8.8.8:5000".parse::<SocketAddr>().unwrap()));
        let resp = create_router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = auth::verify_token(json["token"].as_str().unwrap(), &state.config).unwrap();
        assert!(claims.exp > (chrono::Utc::now() + chrono::Duration::days(300)).timestamp());
    }

    #[tokio::test]
    async fn invalid_token_is_rejected_even_from_lan() {
        let state = auth_state(false).await;
        assert_eq!(
            auth_status(&state, "192.168.1.20:5000", Some("garbage")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn playlist_token_reaches_every_segment_uri() {
        let playlist = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4?p=x&a=0\"\n#EXTINF:6.000000,\nsegment00000.m4s?p=x&a=0\n#EXT-X-ENDLIST\n";
        let out = with_token(playlist, "a.b-c\n#evil");
        assert!(out.contains("#EXT-X-MAP:URI=\"init.mp4?token=a.b-c%0A%23evil&p=x&a=0\""));
        assert!(out.contains("\nsegment00000.m4s?p=x&a=0&token=a.b-c%0A%23evil\n"));
        assert_eq!(out.lines().count(), playlist.lines().count());
    }
}
