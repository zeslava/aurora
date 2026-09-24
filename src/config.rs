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

        Ok(Config {
            port,
            media_paths,
            log_level,
            database_url,
            tmdb_api_key,
            tmdb_language,
            cache_dir,
            strict_auth,
        })
    }
}
