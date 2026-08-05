use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Тип медиа контента
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    /// Фильм
    Movie,
    /// Сериал
    TvSeries,
}

/// Физический медиафайл — единственный носитель информации о файле и кодеках.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFile {
    /// Уникальный идентификатор
    pub id: u64,
    /// Путь к файлу
    pub file_path: PathBuf,
    /// Длительность в секундах
    pub duration: Option<u64>,
    /// Разрешение (например, "1080p", "4K")
    pub resolution: Option<String>,
    /// Видео кодек
    pub video_codec: Option<String>,
    /// Аудио кодек
    pub audio_codec: Option<String>,
    /// Битрэйт видео (бит/сек)
    pub video_bitrate: Option<u64>,
    /// Пиксельный формат видео (например, "yuv420p", "yuv420p10le")
    pub pix_fmt: Option<String>,
}

/// Технические характеристики воспроизводимой сущности — общая часть фильма и
/// эпизода. Берётся из связанного `media_files` и сериализуется плоско
/// (`#[serde(flatten)]`) для совместимости с фронтендом.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaFileInfo {
    /// Путь к файлу (пустой, если файла нет)
    pub file_path: PathBuf,
    /// Длительность в секундах
    pub duration: Option<u64>,
    /// Разрешение (например, "1080p", "4K")
    pub resolution: Option<String>,
    /// Видео кодек
    pub video_codec: Option<String>,
    /// Аудио кодек
    pub audio_codec: Option<String>,
    /// Битрэйт видео (бит/сек)
    pub video_bitrate: Option<u64>,
    /// Пиксельный формат видео
    pub pix_fmt: Option<String>,
}

/// Медиа элемент — контейнер: фильм или сериал.
/// У фильма `file` заполнен, у сериала — пустой (файлы лежат у эпизодов).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Уникальный идентификатор (UUID v7)
    pub id: String,
    /// ID библиотеки
    pub library_id: Option<u64>,
    /// Название (пользовательское, если задано, иначе распарсенное из имени файла)
    pub title: String,
    /// Пользовательское название, если оно задано вручную
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    /// Год выпуска (опционально)
    pub year: Option<u32>,
    /// Тип медиа
    #[serde(rename = "type")]
    pub media_type: MediaType,
    /// Технические характеристики файла
    #[serde(flatten)]
    pub file: MediaFileInfo,
    /// Метаданные TMDB (обложки, описание, рейтинг)
    #[serde(flatten)]
    pub tmdb: TmdbInfo,
    /// Время создания записи
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// Метаданные с TMDB. `poster`/`backdrop` — готовые URL на кэш сервера
/// (`/api/media/:id/poster`), а не ссылки на image.tmdb.org: клиенту не нужен интернет.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TmdbInfo {
    /// ID на TMDB
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<u64>,
    /// Описание
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    /// Рейтинг TMDB (0..10)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// URL постера на этом сервере (None, если обложка не загружена)
    pub poster: Option<String>,
    /// URL фонового кадра на этом сервере
    pub backdrop: Option<String>,
}

/// Библиотека медиа
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    /// Уникальный идентификатор
    pub id: u64,
    /// Название библиотеки
    pub name: String,
    /// Путь к библиотеке
    pub path: PathBuf,
    /// Тип библиотеки
    #[serde(rename = "type")]
    pub library_type: MediaType,
}

/// Метаданные медиафайла, извлеченные через ffprobe
#[derive(Debug, Clone)]
pub struct MediaMetadata {
    /// Длительность в секундах
    pub duration: Option<u64>,
    /// Ширина видео
    pub width: Option<u32>,
    /// Высота видео
    pub height: Option<u32>,
    /// Видео кодек
    pub video_codec: Option<String>,
    /// Аудио кодек
    pub audio_codec: Option<String>,
    /// Битрэйт видео (бит/сек)
    pub video_bitrate: Option<u64>,
    /// Пиксельный формат видео (например, "yuv420p", "yuv420p10le")
    pub pix_fmt: Option<String>,
}

impl MediaMetadata {
    /// Получает разрешение в формате строки (например, "1080p", "4K")
    pub fn resolution(&self) -> Option<String> {
        match (self.width, self.height) {
            (Some(w), Some(h)) => {
                let height = h;
                if height >= 2160 {
                    Some("4K".to_string())
                } else if height >= 1440 {
                    Some("1440p".to_string())
                } else if height >= 1080 {
                    Some("1080p".to_string())
                } else if height >= 720 {
                    Some("720p".to_string())
                } else if height >= 480 {
                    Some("480p".to_string())
                } else {
                    Some(format!("{}x{}", w, h))
                }
            }
            _ => None,
        }
    }
}

/// Эпизод сериала
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Уникальный идентификатор (UUID v7)
    pub id: String,
    /// ID сериала (UUID v7)
    pub series_id: String,
    /// Номер сезона
    pub season_number: u32,
    /// Номер эпизода
    pub episode_number: u32,
    /// Название эпизода
    pub title: Option<String>,
    /// Технические характеристики файла эпизода
    #[serde(flatten)]
    pub file: MediaFileInfo,
    /// Время создания записи
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// Элемент раздела «Продолжить просмотр» на главном экране.
/// `id` — идентификатор воспроизводимой сущности (фильм или эпизод),
/// используется для стриминга и прогресса. Для эпизода заполнены поля сериала.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinueWatchingItem {
    /// ID воспроизводимой сущности (фильм или эпизод, UUID v7)
    pub id: String,
    /// Название (фильма или сериала)
    pub title: String,
    /// Тип: movie или tvseries
    #[serde(rename = "type")]
    pub media_type: MediaType,
    /// Позиция остановки в секундах
    pub position: u64,
    /// Длительность файла в секундах
    pub duration: Option<u64>,
    /// Время последнего просмотра
    pub last_watched: Option<DateTime<Utc>>,
    /// URL постера фильма или сериала на этом сервере
    pub poster: Option<String>,
    /// URL фонового кадра (карточка «Продолжить» — широкая, 16/9)
    pub backdrop: Option<String>,
    /// Для эпизода — ID сериала (для навигации `/watch/:series/:season/:episode`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    /// Для эпизода — номер сезона
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_number: Option<u32>,
    /// Для эпизода — номер эпизода
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<u32>,
    /// Для эпизода — название эпизода
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
}

/// Прогресс просмотра (хранится на уровне файла).
/// Поле `media_id` — это идентификатор из запроса (фильм или эпизод),
/// возвращается как есть для совместимости с фронтендом.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    /// ID пользователя
    pub user_id: u64,
    /// ID медиа из запроса (эхо, UUID v7)
    pub media_id: String,
    /// Позиция в секундах
    pub position: u64,
    /// Просмотрено ли полностью
    pub completed: bool,
    /// Время последнего просмотра
    pub last_watched: Option<DateTime<Utc>>,
}
