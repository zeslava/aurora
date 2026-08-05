use crate::models::{
    ContinueWatchingItem, Episode, Library, MediaFile, MediaFileInfo, MediaItem, MediaType,
    PlaybackState, TmdbInfo,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{
    Row,
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
};
use std::path::PathBuf;
use std::str::FromStr;

/// База данных
#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    /// Создает новое подключение к базе данных
    pub async fn new(database_url: &str) -> Result<Self> {
        use std::path::Path;

        // Убираем префикс "sqlite:" если он есть
        let mut db_path_str = if database_url.starts_with("sqlite:") {
            &database_url[7..]
        } else {
            database_url
        };

        // Убираем "./" в начале пути, если есть
        if db_path_str.starts_with("./") {
            db_path_str = &db_path_str[2..];
        }

        // Разрешаем путь (преобразуем относительный в абсолютный)
        let db_path = if Path::new(db_path_str).is_absolute() {
            Path::new(db_path_str).to_path_buf()
        } else {
            std::env::current_dir()
                .context("Не удалось получить текущую директорию")?
                .join(db_path_str)
        };

        // Создаем родительскую директорию, если она не существует
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).context(format!(
                    "Не удалось создать директорию для БД: {:?}",
                    parent
                ))?;
            }
        }

        // Преобразуем путь в строку для sqlx
        let db_path_str = db_path
            .to_str()
            .context("Путь к БД содержит недопустимые символы")?;

        // Настраиваем опции подключения
        let connect_options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path_str))?
            .create_if_missing(true);

        // Подключаемся к БД
        let pool = SqlitePoolOptions::new()
            .connect_with(connect_options)
            .await
            .context(format!(
                "Не удалось подключиться к базе данных: {}",
                db_path_str
            ))?;

        Ok(Database { pool })
    }

    /// Выполняет миграции
    pub async fn run_migrations(&self) -> Result<()> {
        // Миграции вшиты в бинарь на этапе компиляции: рабочая директория рантайма не важна.
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("Ошибка при выполнении миграций")?;
        Ok(())
    }

    /// Создает библиотеку
    pub async fn create_library(
        &self,
        name: &str,
        path: &PathBuf,
        library_type: &MediaType,
    ) -> Result<u64> {
        let type_str = match library_type {
            MediaType::Movie => "movie",
            MediaType::TvSeries => "tv",
        };

        let result =
            sqlx::query("INSERT INTO libraries (name, path, type) VALUES (?, ?, ?) RETURNING id")
                .bind(name)
                .bind(path.to_string_lossy().as_ref())
                .bind(type_str)
                .fetch_one(&self.pool)
                .await
                .context("Не удалось создать библиотеку")?;

        Ok(result.get::<i64, _>(0) as u64)
    }

    /// Получает все библиотеки
    pub async fn get_libraries(&self) -> Result<Vec<Library>> {
        let rows = sqlx::query("SELECT id, name, path, type FROM libraries")
            .fetch_all(&self.pool)
            .await
            .context("Не удалось получить библиотеки")?;

        let libraries: Result<Vec<Library>> = rows
            .into_iter()
            .map(|row| {
                let id = row.get::<i64, _>(0) as u64;
                let name = row.get::<String, _>(1);
                let path_str = row.get::<String, _>(2);
                let type_str = row.get::<String, _>(3);

                let library_type = match type_str.as_str() {
                    "movie" => MediaType::Movie,
                    "tv" => MediaType::TvSeries,
                    _ => MediaType::Movie,
                };

                Ok(Library {
                    id,
                    name,
                    path: PathBuf::from(path_str),
                    library_type,
                })
            })
            .collect();

        libraries
    }

    /// Вставляет физический файл, возвращает его id
    pub async fn insert_media_file(&self, file: &MediaFile) -> Result<u64> {
        let result = sqlx::query(
            "INSERT INTO media_files (file_path, duration, resolution, video_codec, audio_codec, video_bitrate, pix_fmt)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(file.file_path.to_string_lossy().as_ref())
        .bind(file.duration.map(|d| d as i64))
        .bind(&file.resolution)
        .bind(&file.video_codec)
        .bind(&file.audio_codec)
        .bind(file.video_bitrate.map(|b| b as i64))
        .bind(&file.pix_fmt)
        .fetch_one(&self.pool)
        .await
        .context("Не удалось вставить медиафайл")?;

        Ok(result.get::<i64, _>(0) as u64)
    }

    /// Проверяет, есть ли уже файл с таким путём
    pub async fn media_file_exists(&self, file_path: &PathBuf) -> Result<bool> {
        let result = sqlx::query("SELECT COUNT(*) FROM media_files WHERE file_path = ?")
            .bind(file_path.to_string_lossy().as_ref())
            .fetch_one(&self.pool)
            .await
            .context("Не удалось проверить существование файла")?;

        let count: i64 = result.get(0);
        Ok(count > 0)
    }

    /// id уже известного файла по пути
    pub async fn media_file_id(&self, file_path: &PathBuf) -> Result<Option<u64>> {
        let row = sqlx::query("SELECT id FROM media_files WHERE file_path = ?")
            .bind(file_path.to_string_lossy().as_ref())
            .fetch_optional(&self.pool)
            .await
            .context("Не удалось найти файл")?;

        Ok(row.map(|r| r.get::<i64, _>(0) as u64))
    }

    /// Перепривязывает уже известный файл-эпизод к сериалу и номерам,
    /// которые выдал текущий парсер. Возвращает true при изменении.
    pub async fn relink_episode(
        &self,
        file_id: u64,
        series_id: &str,
        season_number: u32,
        episode_number: u32,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE episodes
             SET series_id = ?, season_number = ?, episode_number = ?
             WHERE file_id = ?
               AND (series_id <> ? OR season_number <> ? OR episode_number <> ?)",
        )
        .bind(series_id)
        .bind(season_number as i64)
        .bind(episode_number as i64)
        .bind(file_id as i64)
        .bind(series_id)
        .bind(season_number as i64)
        .bind(episode_number as i64)
        .execute(&self.pool)
        .await
        .context("Не удалось перепривязать эпизод")?;

        Ok(result.rows_affected() > 0)
    }

    /// Есть ли эпизод, привязанный к этому файлу
    pub async fn episode_exists_for_file(&self, file_id: u64) -> Result<bool> {
        let row = sqlx::query("SELECT COUNT(*) FROM episodes WHERE file_id = ?")
            .bind(file_id as i64)
            .fetch_one(&self.pool)
            .await
            .context("Не удалось проверить эпизод по файлу")?;

        Ok(row.get::<i64, _>(0) > 0)
    }

    /// Обновляет название/год фильма по пути файла, если парсер выдал другое значение.
    /// Сбрасывает отметку TMDB, чтобы метаданные перезапросились. Возвращает true при изменении.
    pub async fn refresh_movie_title(
        &self,
        file_path: &PathBuf,
        title: &str,
        year: Option<u32>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE media_items
             SET title = ?, year = COALESCE(?, year), tmdb_synced_at = NULL
             WHERE file_id = (SELECT id FROM media_files WHERE file_path = ?)
               AND (title <> ? OR (? IS NOT NULL AND year IS NOT ?))",
        )
        .bind(title)
        .bind(year.map(|y| y as i64))
        .bind(file_path.to_string_lossy().as_ref())
        .bind(title)
        .bind(year.map(|y| y as i64))
        .bind(year.map(|y| y as i64))
        .execute(&self.pool)
        .await
        .context("Не удалось обновить название фильма")?;

        Ok(result.rows_affected() > 0)
    }

    /// Удаляет из БД файлы библиотеки, которых больше нет на диске.
    /// `present` — пути, найденные текущим сканированием этой библиотеки.
    /// Каскадом уходят эпизоды и прогресс просмотра; осиротевшие фильмы и
    /// сериалы без эпизодов подчищаются следом. Возвращает число удалённых файлов.
    pub async fn prune_missing_files(
        &self,
        library_path: &std::path::Path,
        present: &std::collections::HashSet<PathBuf>,
    ) -> Result<u64> {
        // Префикс с разделителем, чтобы "/media/tv" не зацепил "/media/tv2"
        let prefix = format!("{}/", library_path.to_string_lossy().trim_end_matches('/'));

        let rows = sqlx::query("SELECT id, file_path FROM media_files")
            .fetch_all(&self.pool)
            .await
            .context("Не удалось получить файлы библиотеки")?;

        let stale: Vec<i64> = rows
            .into_iter()
            .filter_map(|r| {
                let raw = r.get::<String, _>(1);
                if !raw.starts_with(&prefix) {
                    return None;
                }
                (!present.contains(&PathBuf::from(raw))).then(|| r.get::<i64, _>(0))
            })
            .collect();

        if stale.is_empty() {
            return Ok(0);
        }

        let mut deleted = 0u64;
        for chunk in stale.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("DELETE FROM media_files WHERE id IN ({placeholders})");
            let mut query = sqlx::query(&sql);
            for id in chunk {
                query = query.bind(id);
            }
            deleted += query
                .execute(&self.pool)
                .await
                .context("Не удалось удалить исчезнувшие файлы")?
                .rows_affected();
        }

        self.cleanup_orphans().await?;

        Ok(deleted)
    }

    /// Удаляет фильмы без файла и сериалы без эпизодов
    pub async fn cleanup_orphans(&self) -> Result<u64> {
        // Фильм без файла, либо фильм, чей файл после переcканирования стал эпизодом:
        // парсер имён поумнел, и старая карточка осталась дублем рядом с сериалом.
        // Прогресс просмотра живёт на file_id, поэтому при удалении карточки не теряется.
        let movies = sqlx::query(
            "DELETE FROM media_items
             WHERE media_type = 'movie'
               AND (file_id IS NULL
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.file_id = media_items.file_id))",
        )
        .execute(&self.pool)
        .await
        .context("Не удалось удалить фильмы без файлов")?
        .rows_affected();

        let series = sqlx::query(
            "DELETE FROM media_items
             WHERE media_type = 'tv'
               AND NOT EXISTS (SELECT 1 FROM episodes e WHERE e.series_id = media_items.id)",
        )
        .execute(&self.pool)
        .await
        .context("Не удалось удалить сериалы без эпизодов")?
        .rows_affected();

        Ok(movies + series)
    }

    /// Резолвит физический файл по id медиа (сначала как эпизод, затем как фильм).
    /// Используется стримингом и прогрессом.
    pub async fn resolve_media_file(&self, id: &str) -> Result<Option<MediaFile>> {
        let row = sqlx::query(&format!(
            "SELECT {FILE_COLUMNS}
             FROM media_files mf JOIN episodes e ON e.file_id = mf.id WHERE e.id = ?
             UNION ALL
             SELECT {FILE_COLUMNS}
             FROM media_files mf JOIN media_items mi ON mi.file_id = mf.id WHERE mi.id = ?
             LIMIT 1"
        ))
        .bind(id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось резолвить медиафайл")?;

        Ok(row.map(row_to_media_file))
    }

    /// Возвращает закэшированную карту сегментов (JSON границ) или None.
    pub async fn get_segment_map(&self, file_id: u64, profile: &str) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT boundaries FROM media_file_segments WHERE file_id = ? AND profile = ?",
        )
        .bind(file_id as i64)
        .bind(profile)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось прочитать карту сегментов")?;

        Ok(row.map(|r| r.get::<String, _>("boundaries")))
    }

    /// Сохраняет карту сегментов; при повторе перезаписывает (INSERT OR REPLACE).
    pub async fn save_segment_map(
        &self,
        file_id: u64,
        profile: &str,
        boundaries: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO media_file_segments (file_id, profile, boundaries) VALUES (?, ?, ?)",
        )
        .bind(file_id as i64)
        .bind(profile)
        .bind(boundaries)
        .execute(&self.pool)
        .await
        .context("Не удалось сохранить карту сегментов")?;

        Ok(())
    }

    /// Вставляет медиа элемент (фильм или контейнер-сериал)
    pub async fn insert_media_item(
        &self,
        library_id: Option<u64>,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
        file_id: Option<u64>,
    ) -> Result<String> {
        let type_str = media_type_str(media_type);
        let id = new_entity_id();

        sqlx::query(
            "INSERT INTO media_items (id, library_id, title, year, media_type, file_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(library_id.map(|id| id as i64))
        .bind(title)
        .bind(year.map(|y| y as i64))
        .bind(type_str)
        .bind(file_id.map(|id| id as i64))
        .execute(&self.pool)
        .await
        .context("Не удалось вставить медиа элемент")?;

        Ok(id)
    }

    /// Ищет сериал по названию в библиотеке (для дедупликации при сканировании)
    pub async fn find_series_by_title(
        &self,
        library_id: Option<u64>,
        title: &str,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT id FROM media_items
             WHERE media_type = 'tv' AND title = ? AND library_id IS ?",
        )
        .bind(title)
        .bind(library_id.map(|id| id as i64))
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось найти сериал")?;

        Ok(row.map(|r| r.get::<String, _>(0)))
    }

    /// Получает медиа элементы с фильтрацией
    pub async fn get_media_items(&self, media_type: Option<&MediaType>) -> Result<Vec<MediaItem>> {
        let base = format!(
            "SELECT mi.id, mi.library_id, COALESCE(mi.custom_title, mi.title), mi.year, mi.media_type, mi.created_at, {FILE_COLUMNS}, {TMDB_COLUMNS}, mi.custom_title \
             FROM media_items mi LEFT JOIN media_files mf ON mi.file_id = mf.id"
        );

        let filtered = format!("{base} WHERE mi.media_type = ?");
        let query = if let Some(mt) = media_type {
            sqlx::query(&filtered).bind(media_type_str(mt))
        } else {
            sqlx::query(&base)
        };

        let rows = query
            .fetch_all(&self.pool)
            .await
            .context("Не удалось получить медиа элементы")?;

        Ok(rows.into_iter().map(row_to_media_item).collect())
    }

    /// Получает эпизод по ID (используется тестом на непересечение id с фильмами)
    #[cfg(test)]
    pub async fn get_episode_by_id(&self, episode_id: &str) -> Result<Option<Episode>> {
        let row = sqlx::query(&format!(
            "SELECT e.id, e.series_id, e.season_number, e.episode_number, e.title, e.created_at, {FILE_COLUMNS}
             FROM episodes e JOIN media_files mf ON e.file_id = mf.id
             WHERE e.id = ?"
        ))
        .bind(episode_id)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось получить эпизод")?;

        Ok(row.map(row_to_episode))
    }

    /// Вставляет эпизод
    pub async fn insert_episode(
        &self,
        series_id: &str,
        season_number: u32,
        episode_number: u32,
        title: Option<&str>,
        file_id: u64,
    ) -> Result<String> {
        let id = new_entity_id();

        sqlx::query(
            "INSERT INTO episodes (id, series_id, season_number, episode_number, title, file_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(series_id)
        .bind(season_number as i64)
        .bind(episode_number as i64)
        .bind(title)
        .bind(file_id as i64)
        .execute(&self.pool)
        .await
        .context("Не удалось вставить эпизод")?;

        Ok(id)
    }

    /// Получает эпизоды сериала
    pub async fn get_episodes(&self, series_id: &str) -> Result<Vec<Episode>> {
        let rows = sqlx::query(&format!(
            "SELECT e.id, e.series_id, e.season_number, e.episode_number, e.title, e.created_at, {FILE_COLUMNS}
             FROM episodes e JOIN media_files mf ON e.file_id = mf.id
             WHERE e.series_id = ?
             ORDER BY e.season_number, e.episode_number"
        ))
        .bind(series_id)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить эпизоды")?;

        Ok(rows.into_iter().map(row_to_episode).collect())
    }

    /// Обновляет или создает состояние просмотра (по файлу)
    pub async fn upsert_playback_state(
        &self,
        user_id: u64,
        file_id: u64,
        position: u64,
        completed: bool,
        last_watched: Option<DateTime<Utc>>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO playback_state (user_id, file_id, position, completed, last_watched)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, file_id)
             DO UPDATE SET position = ?, completed = ?, last_watched = ?",
        )
        .bind(user_id as i64)
        .bind(file_id as i64)
        .bind(position as i64)
        .bind(completed)
        .bind(last_watched.map(|dt| dt.to_rfc3339()))
        .bind(position as i64)
        .bind(completed)
        .bind(last_watched.map(|dt| dt.to_rfc3339()))
        .execute(&self.pool)
        .await
        .context("Не удалось обновить состояние просмотра")?;

        Ok(())
    }

    /// Получает состояние просмотра по файлу.
    /// `media_id` в результате — это переданный `media_id` (эхо для фронтенда).
    pub async fn get_playback_state(
        &self,
        user_id: u64,
        file_id: u64,
        media_id: String,
    ) -> Result<Option<PlaybackState>> {
        let row = sqlx::query(
            "SELECT position, completed, last_watched
             FROM playback_state
             WHERE user_id = ? AND file_id = ?",
        )
        .bind(user_id as i64)
        .bind(file_id as i64)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось получить состояние просмотра")?;

        if let Some(row) = row {
            let position = row.get::<i64, _>(0) as u64;
            let completed = row.get::<bool, _>(1);
            let last_watched: Option<DateTime<Utc>> = row
                .get::<Option<String>, _>(2)
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            Ok(Some(PlaybackState {
                user_id,
                media_id,
                position,
                completed,
                last_watched,
            }))
        } else {
            Ok(None)
        }
    }

    /// «Продолжить просмотр»: незавершённые файлы пользователя (0 < position < duration),
    /// сопоставленные с фильмом или эпизодом, отсортированные по времени просмотра.
    pub async fn get_continue_watching(
        &self,
        user_id: u64,
        limit: i64,
    ) -> Result<Vec<ContinueWatchingItem>> {
        // Фильмы
        let movie_rows = sqlx::query(
            "SELECT mi.id, COALESCE(mi.custom_title, mi.title), ps.position, mf.duration, ps.last_watched, mi.poster_file, mi.backdrop_file
             FROM playback_state ps
             JOIN media_files mf ON mf.id = ps.file_id
             JOIN media_items mi ON mi.file_id = mf.id
             WHERE ps.user_id = ? AND ps.completed = 0
               AND ps.position > 0
               AND (mf.duration IS NULL OR ps.position < mf.duration)",
        )
        .bind(user_id as i64)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить продолжение (фильмы)")?;

        // Эпизоды
        let episode_rows = sqlx::query(
            "SELECT e.id, COALESCE(s.custom_title, s.title), ps.position, mf.duration, ps.last_watched,
                    e.series_id, e.season_number, e.episode_number, e.title AS episode_title,
                    s.poster_file, s.backdrop_file
             FROM playback_state ps
             JOIN media_files mf ON mf.id = ps.file_id
             JOIN episodes e ON e.file_id = mf.id
             JOIN media_items s ON s.id = e.series_id
             WHERE ps.user_id = ? AND ps.completed = 0
               AND ps.position > 0
               AND (mf.duration IS NULL OR ps.position < mf.duration)",
        )
        .bind(user_id as i64)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить продолжение (эпизоды)")?;

        let mut items: Vec<ContinueWatchingItem> = Vec::new();

        for row in movie_rows {
            let id = row.get::<String, _>(0);
            items.push(ContinueWatchingItem {
                poster: image_url(&id, row.get::<Option<String>, _>(5).as_deref(), "poster"),
                backdrop: image_url(&id, row.get::<Option<String>, _>(6).as_deref(), "backdrop"),
                id,
                title: row.get::<String, _>(1),
                media_type: MediaType::Movie,
                position: row.get::<i64, _>(2) as u64,
                duration: row.get::<Option<i64>, _>(3).map(|d| d as u64),
                last_watched: parse_created_at(row.get::<Option<String>, _>(4)),
                series_id: None,
                season_number: None,
                episode_number: None,
                episode_title: None,
            });
        }

        for row in episode_rows {
            // Обложка у эпизода — постер сериала, поэтому URL строится по series_id
            let series_id = row.get::<String, _>(5);
            items.push(ContinueWatchingItem {
                poster: image_url(
                    &series_id,
                    row.get::<Option<String>, _>(9).as_deref(),
                    "poster",
                ),
                backdrop: image_url(
                    &series_id,
                    row.get::<Option<String>, _>(10).as_deref(),
                    "backdrop",
                ),
                id: row.get::<String, _>(0),
                title: row.get::<String, _>(1),
                media_type: MediaType::TvSeries,
                position: row.get::<i64, _>(2) as u64,
                duration: row.get::<Option<i64>, _>(3).map(|d| d as u64),
                last_watched: parse_created_at(row.get::<Option<String>, _>(4)),
                series_id: Some(series_id),
                season_number: Some(row.get::<i64, _>(6) as u32),
                episode_number: Some(row.get::<i64, _>(7) as u32),
                episode_title: row.get::<Option<String>, _>(8),
            });
        }

        // Сортировка по времени просмотра (свежие сверху), затем ограничение.
        items.sort_by(|a, b| b.last_watched.cmp(&a.last_watched));
        items.truncate(limit.max(0) as usize);

        Ok(items)
    }

    /// «Недавно добавленное»: фильмы и сериалы по времени создания записи.
    pub async fn get_recently_added(&self, limit: i64) -> Result<Vec<MediaItem>> {
        let rows = sqlx::query(&format!(
            "SELECT mi.id, mi.library_id, COALESCE(mi.custom_title, mi.title), mi.year, mi.media_type, mi.created_at, {FILE_COLUMNS}, {TMDB_COLUMNS}, mi.custom_title \
             FROM media_items mi LEFT JOIN media_files mf ON mi.file_id = mf.id \
             ORDER BY mi.created_at DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить недавно добавленное")?;

        Ok(rows.into_iter().map(row_to_media_item).collect())
    }

    /// Создает нового пользователя
    pub async fn create_user(&self, username: &str, password_hash: &str) -> Result<u64> {
        let result =
            sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING id")
                .bind(username)
                .bind(password_hash)
                .fetch_one(&self.pool)
                .await
                .context("Не удалось создать пользователя")?;

        Ok(result.get::<i64, _>(0) as u64)
    }

    /// Получает пользователя по имени
    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<(u64, String)>> {
        let row = sqlx::query("SELECT id, password_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .context("Не удалось получить пользователя")?;

        if let Some(row) = row {
            let id = row.get::<i64, _>(0) as u64;
            let password_hash = row.get::<String, _>(1);
            Ok(Some((id, password_hash)))
        } else {
            Ok(None)
        }
    }

    /// Получает медиа элемент по ID
    pub async fn get_media_item_by_id(&self, id: &str) -> Result<Option<MediaItem>> {
        let row = sqlx::query(&format!(
            "SELECT mi.id, mi.library_id, COALESCE(mi.custom_title, mi.title), mi.year, mi.media_type, mi.created_at, {FILE_COLUMNS}, {TMDB_COLUMNS}, mi.custom_title \
             FROM media_items mi LEFT JOIN media_files mf ON mi.file_id = mf.id WHERE mi.id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось получить медиа элемент")?;

        Ok(row.map(row_to_media_item))
    }

    /// Элементы, для которых ещё не пробовали получить метаданные TMDB.
    /// Возвращает (id, title, year, media_type).
    pub async fn get_items_pending_tmdb(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<u32>, MediaType)>> {
        let rows = sqlx::query(
            "SELECT id, COALESCE(custom_title, title), year, media_type FROM media_items
             WHERE tmdb_synced_at IS NULL
             ORDER BY created_at LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить элементы без метаданных TMDB")?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let media_type = match row.get::<String, _>(3).as_str() {
                    "tv" => MediaType::TvSeries,
                    _ => MediaType::Movie,
                };
                (
                    row.get::<String, _>(0),
                    row.get::<String, _>(1),
                    row.get::<Option<i64>, _>(2).map(|y| y as u32),
                    media_type,
                )
            })
            .collect())
    }

    /// Сохраняет метаданные TMDB и отмечает элемент как синхронизированный
    pub async fn update_tmdb_metadata(
        &self,
        id: &str,
        tmdb_id: u64,
        overview: Option<&str>,
        rating: Option<f64>,
        poster_file: Option<&str>,
        backdrop_file: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE media_items
             SET tmdb_id = ?, overview = ?, rating = ?,
                 poster_file = COALESCE(?, poster_file),
                 backdrop_file = COALESCE(?, backdrop_file),
                 tmdb_synced_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(tmdb_id as i64)
        .bind(overview)
        .bind(rating)
        .bind(poster_file)
        .bind(backdrop_file)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("Не удалось сохранить метаданные TMDB")?;

        Ok(())
    }

    /// Отмечает попытку синхронизации без результата, чтобы не долбить API
    /// одним и тем же ненайденным названием на каждом скане
    pub async fn mark_tmdb_synced(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE media_items SET tmdb_synced_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Не удалось отметить синхронизацию TMDB")?;

        Ok(())
    }

    /// Имя закэшированного файла обложки (`kind` — "poster" или "backdrop")
    pub async fn get_image_file(&self, id: &str, kind: &str) -> Result<Option<String>> {
        let column = match kind {
            "poster" => "poster_file",
            "backdrop" => "backdrop_file",
            _ => return Ok(None),
        };

        let row = sqlx::query(&format!("SELECT {column} FROM media_items WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .context("Не удалось получить файл обложки")?;

        Ok(row.and_then(|r| r.get::<Option<String>, _>(0)))
    }

    /// Задаёт (или снимает, если `None`) пользовательское название.
    /// Сбрасывает `tmdb_synced_at`: фоновое обогащение перезапросит TMDB по новому названию.
    /// Возвращает false, если элемента нет.
    pub async fn set_custom_title(&self, id: &str, custom_title: Option<&str>) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE media_items SET custom_title = ?, tmdb_synced_at = NULL WHERE id = ?",
        )
        .bind(custom_title)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("Не удалось сохранить пользовательское название")?;

        Ok(result.rows_affected() > 0)
    }

    /// Номера сезонов, для которых у сериала есть эпизоды
    pub async fn get_season_numbers(&self, series_id: &str) -> Result<Vec<u32>> {
        let rows = sqlx::query(
            "SELECT DISTINCT season_number FROM episodes WHERE series_id = ? ORDER BY season_number",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить сезоны сериала")?;

        Ok(rows
            .into_iter()
            .map(|r| r.get::<i64, _>(0) as u32)
            .collect())
    }

    /// Сохраняет постер сезона
    pub async fn upsert_season_image(
        &self,
        series_id: &str,
        season_number: u32,
        poster_file: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO season_images (series_id, season_number, poster_file)
             VALUES (?, ?, ?)
             ON CONFLICT(series_id, season_number)
             DO UPDATE SET poster_file = excluded.poster_file",
        )
        .bind(series_id)
        .bind(season_number as i64)
        .bind(poster_file)
        .execute(&self.pool)
        .await
        .context("Не удалось сохранить постер сезона")?;

        Ok(())
    }

    /// Имя закэшированного файла постера сезона
    pub async fn get_season_image_file(
        &self,
        series_id: &str,
        season_number: u32,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT poster_file FROM season_images WHERE series_id = ? AND season_number = ?",
        )
        .bind(series_id)
        .bind(season_number as i64)
        .fetch_optional(&self.pool)
        .await
        .context("Не удалось получить постер сезона")?;

        Ok(row.and_then(|r| r.get::<Option<String>, _>(0)))
    }

    /// URL постеров сезонов сериала, по номеру сезона
    pub async fn get_season_posters(&self, series_id: &str) -> Result<Vec<(u32, String)>> {
        let rows = sqlx::query(
            "SELECT season_number, poster_file FROM season_images
             WHERE series_id = ? AND poster_file IS NOT NULL",
        )
        .bind(series_id)
        .fetch_all(&self.pool)
        .await
        .context("Не удалось получить постеры сезонов")?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let season = r.get::<i64, _>(0) as u32;
                let file = r.get::<String, _>(1);
                (
                    season,
                    format!("/api/series/{series_id}/seasons/{season}/poster?v={file}"),
                )
            })
            .collect())
    }

    /// Удаляет постеры сезонов сериала, возвращая имена файлов для чистки кэша
    pub async fn clear_season_images(&self, series_id: &str) -> Result<Vec<String>> {
        let rows =
            sqlx::query("DELETE FROM season_images WHERE series_id = ? RETURNING poster_file")
                .bind(series_id)
                .fetch_all(&self.pool)
                .await
                .context("Не удалось удалить постеры сезонов")?;

        Ok(rows
            .into_iter()
            .filter_map(|r| r.get::<Option<String>, _>(0))
            .collect())
    }

    /// Сбрасывает отметки синхронизации — следующий проход перезапросит TMDB
    pub async fn reset_tmdb_sync(&self) -> Result<u64> {
        let result = sqlx::query("UPDATE media_items SET tmdb_synced_at = NULL")
            .execute(&self.pool)
            .await
            .context("Не удалось сбросить синхронизацию TMDB")?;

        Ok(result.rows_affected())
    }
}

/// Генерирует новый идентификатор сущности (UUID v7, отсортирован по времени).
fn new_entity_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn media_type_str(media_type: &MediaType) -> &'static str {
    match media_type {
        MediaType::Movie => "movie",
        MediaType::TvSeries => "tv",
    }
}

fn parse_created_at(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn row_to_media_file(row: sqlx::sqlite::SqliteRow) -> MediaFile {
    MediaFile {
        id: row.get::<i64, _>(0) as u64,
        file_path: PathBuf::from(row.get::<String, _>(1)),
        duration: row.get::<Option<i64>, _>(2).map(|d| d as u64),
        resolution: row.get::<Option<String>, _>(3),
        video_codec: row.get::<Option<String>, _>(4),
        audio_codec: row.get::<Option<String>, _>(5),
        video_bitrate: row.get::<Option<i64>, _>(6).map(|b| b as u64),
        pix_fmt: row.get::<Option<String>, _>(7),
    }
}

/// Общая часть SELECT'а для полей файла — порядок колонок фиксирован и разбирается
/// в `row_to_file_info` начиная с `offset`.
const FILE_COLUMNS: &str = "mf.id, mf.file_path, mf.duration, mf.resolution, \
                            mf.video_codec, mf.audio_codec, mf.video_bitrate, mf.pix_fmt";

/// Колонки метаданных TMDB — идут в SELECT'ах сразу после `FILE_COLUMNS`
/// и разбираются в `row_to_tmdb_info`.
const TMDB_COLUMNS: &str = "mi.tmdb_id, mi.overview, mi.rating, mi.poster_file, mi.backdrop_file";

/// Строит URL картинки на этом сервере; None, если файл не закэширован.
/// `?v=` — имя закэшированного файла: оно меняется вместе с картинкой, иначе
/// клиент неделю показывал бы старую обложку из своего кэша.
fn image_url(media_id: &str, file: Option<&str>, kind: &str) -> Option<String> {
    file.map(|file| format!("/api/media/{media_id}/{kind}?v={file}"))
}

/// Разбирает 5 колонок TMDB (`TMDB_COLUMNS`), начиная с `offset`.
fn row_to_tmdb_info(row: &sqlx::sqlite::SqliteRow, offset: usize, media_id: &str) -> TmdbInfo {
    TmdbInfo {
        tmdb_id: row.get::<Option<i64>, _>(offset).map(|id| id as u64),
        overview: row.get::<Option<String>, _>(offset + 1),
        rating: row.get::<Option<f64>, _>(offset + 2),
        poster: image_url(
            media_id,
            row.get::<Option<String>, _>(offset + 3).as_deref(),
            "poster",
        ),
        backdrop: image_url(
            media_id,
            row.get::<Option<String>, _>(offset + 4).as_deref(),
            "backdrop",
        ),
    }
}

/// Разбирает 8 колонок файла (`FILE_COLUMNS`), начиная с `offset`.
/// Колонка `mf.id` (по смещению `offset`) не нужна наружу и пропускается.
fn row_to_file_info(row: &sqlx::sqlite::SqliteRow, offset: usize) -> MediaFileInfo {
    MediaFileInfo {
        file_path: row
            .get::<Option<String>, _>(offset + 1)
            .map(PathBuf::from)
            .unwrap_or_default(),
        duration: row.get::<Option<i64>, _>(offset + 2).map(|d| d as u64),
        resolution: row.get::<Option<String>, _>(offset + 3),
        video_codec: row.get::<Option<String>, _>(offset + 4),
        audio_codec: row.get::<Option<String>, _>(offset + 5),
        video_bitrate: row.get::<Option<i64>, _>(offset + 6).map(|b| b as u64),
        pix_fmt: row.get::<Option<String>, _>(offset + 7),
    }
}

/// Ожидает колонки: id, library_id, title, year, media_type, created_at,
/// затем `FILE_COLUMNS` (8 колонок), `TMDB_COLUMNS` (5 колонок) и custom_title
fn row_to_media_item(row: sqlx::sqlite::SqliteRow) -> MediaItem {
    let media_type = match row.get::<String, _>(4).as_str() {
        "tv" => MediaType::TvSeries,
        _ => MediaType::Movie,
    };
    let id = row.get::<String, _>(0);

    MediaItem {
        tmdb: row_to_tmdb_info(&row, 14, &id),
        id,
        library_id: row.get::<Option<i64>, _>(1).map(|id| id as u64),
        title: row.get::<String, _>(2),
        custom_title: row.get::<Option<String>, _>(19),
        year: row.get::<Option<i64>, _>(3).map(|y| y as u32),
        media_type,
        created_at: parse_created_at(row.get::<Option<String>, _>(5)),
        file: row_to_file_info(&row, 6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Создаёт БД в памяти со схемой из миграции.
    async fn test_db() -> Database {
        let pool = SqlitePoolOptions::new()
            .max_connections(1) // in-memory БД живёт в рамках одного соединения
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

        Database { pool }
    }

    async fn insert_file(db: &Database, path: &str) -> u64 {
        db.insert_media_file(&MediaFile {
            id: 0,
            file_path: PathBuf::from(path),
            duration: Some(100),
            resolution: None,
            video_codec: None,
            audio_codec: None,
            video_bitrate: None,
            pix_fmt: None,
        })
        .await
        .unwrap()
    }

    /// Регрессия: id фильма и эпизода не должны совпадать, иначе стриминг
    /// по /api/media/:id отдавал чужой контент.
    #[tokio::test]
    async fn movie_and_episode_ids_do_not_collide() {
        let db = test_db().await;

        let movie_file = insert_file(&db, "/media/movie.mp4").await;
        let movie_id = db
            .insert_media_item(
                None,
                "Фильм",
                Some(2020),
                &MediaType::Movie,
                Some(movie_file),
            )
            .await
            .unwrap();

        let series_id = db
            .insert_media_item(None, "Сериал", None, &MediaType::TvSeries, None)
            .await
            .unwrap();
        let ep_file = insert_file(&db, "/media/ep.mp4").await;
        let episode_id = db
            .insert_episode(&series_id, 1, 1, None, ep_file)
            .await
            .unwrap();

        assert_ne!(movie_id, episode_id);
        // Все id — валидные UUID.
        for id in [&movie_id, &series_id, &episode_id] {
            assert!(uuid::Uuid::parse_str(id).is_ok(), "не UUID: {id}");
        }
    }

    /// Резолвинг файла по id должен отдавать именно тот файл, что привязан к сущности.
    #[tokio::test]
    async fn resolve_media_file_returns_correct_file() {
        let db = test_db().await;

        let movie_file = insert_file(&db, "/media/movie.mp4").await;
        let movie_id = db
            .insert_media_item(None, "Фильм", None, &MediaType::Movie, Some(movie_file))
            .await
            .unwrap();

        let series_id = db
            .insert_media_item(None, "Сериал", None, &MediaType::TvSeries, None)
            .await
            .unwrap();
        let ep_file = insert_file(&db, "/media/ep.mp4").await;
        let episode_id = db
            .insert_episode(&series_id, 1, 1, None, ep_file)
            .await
            .unwrap();

        let by_movie = db.resolve_media_file(&movie_id).await.unwrap().unwrap();
        assert_eq!(by_movie.file_path, PathBuf::from("/media/movie.mp4"));

        let by_episode = db.resolve_media_file(&episode_id).await.unwrap().unwrap();
        assert_eq!(by_episode.file_path, PathBuf::from("/media/ep.mp4"));

        // id фильма не должен резолвиться как эпизод и наоборот.
        assert!(db.get_episode_by_id(&movie_id).await.unwrap().is_none());
        let ep = db.get_episode_by_id(&episode_id).await.unwrap().unwrap();
        assert_eq!(ep.file.file_path, PathBuf::from("/media/ep.mp4"));
    }

    #[tokio::test]
    async fn segment_map_roundtrip() {
        let db = test_db().await;
        let file_id = insert_file(&db, "/media/movie.mp4").await;

        assert!(db.get_segment_map(file_id, "copy").await.unwrap().is_none());

        db.save_segment_map(file_id, "copy", "[0.0,4.0,8.0]")
            .await
            .unwrap();
        assert_eq!(
            db.get_segment_map(file_id, "copy").await.unwrap().unwrap(),
            "[0.0,4.0,8.0]"
        );

        db.save_segment_map(file_id, "copy", "[0.0,5.0]")
            .await
            .unwrap();
        assert_eq!(
            db.get_segment_map(file_id, "copy").await.unwrap().unwrap(),
            "[0.0,5.0]"
        );
    }

    /// Пропавшие с диска файлы удаляются вместе с фильмами и опустевшими сериалами,
    /// файлы вне сканируемой библиотеки — не трогаются.
    #[tokio::test]
    async fn prune_removes_only_missing_files_of_library() {
        let db = test_db().await;

        let kept_file = insert_file(&db, "/media/movies/kept.mp4").await;
        db.insert_media_item(None, "Остался", None, &MediaType::Movie, Some(kept_file))
            .await
            .unwrap();
        let gone_file = insert_file(&db, "/media/movies/gone.mp4").await;
        db.insert_media_item(None, "Пропал", None, &MediaType::Movie, Some(gone_file))
            .await
            .unwrap();
        // Другая библиотека с похожим префиксом — не должна пострадать
        let other_file = insert_file(&db, "/media/movies2/other.mp4").await;
        db.insert_media_item(None, "Чужой", None, &MediaType::Movie, Some(other_file))
            .await
            .unwrap();

        let series_id = db
            .insert_media_item(None, "Сериал", None, &MediaType::TvSeries, None)
            .await
            .unwrap();
        let ep_file = insert_file(&db, "/media/movies/s01e01.mp4").await;
        db.insert_episode(&series_id, 1, 1, None, ep_file)
            .await
            .unwrap();

        let present = std::collections::HashSet::from([PathBuf::from("/media/movies/kept.mp4")]);
        let deleted = db
            .prune_missing_files(std::path::Path::new("/media/movies"), &present)
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        let titles: Vec<String> = db
            .get_media_items(None)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(titles, vec!["Остался".to_string(), "Чужой".to_string()]);
        assert!(db.get_episodes(&series_id).await.unwrap().is_empty());
    }
}

/// Ожидает колонки: id, series_id, season_number, episode_number, title, created_at,
/// затем `FILE_COLUMNS`
fn row_to_episode(row: sqlx::sqlite::SqliteRow) -> Episode {
    Episode {
        id: row.get::<String, _>(0),
        series_id: row.get::<String, _>(1),
        season_number: row.get::<i64, _>(2) as u32,
        episode_number: row.get::<i64, _>(3) as u32,
        title: row.get::<Option<String>, _>(4),
        created_at: parse_created_at(row.get::<Option<String>, _>(5)),
        file: row_to_file_info(&row, 6),
    }
}
