-- Пользователи (пока один админ)
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Библиотеки (пути к медиа)
CREATE TABLE IF NOT EXISTS libraries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    path TEXT UNIQUE NOT NULL,
    type TEXT NOT NULL, -- 'movie' или 'tv'
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Физические файлы: единственный носитель информации о файле и кодеках.
-- На один файл ссылается либо фильм (media_items.file_id), либо эпизод (episodes.file_id).
CREATE TABLE IF NOT EXISTS media_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT UNIQUE NOT NULL,
    duration INTEGER, -- в секундах
    resolution TEXT,
    video_codec TEXT,
    audio_codec TEXT,
    video_bitrate INTEGER, -- бит/сек
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Медиа элемент — контейнер: фильм или сериал.
-- Для фильма file_id указывает на media_files; для сериала file_id = NULL (файлы у эпизодов).
-- id — UUID v7 (TEXT), чтобы глобально не пересекаться с id эпизодов: иначе
-- стриминг по /api/media/:id подменял бы фильм эпизодом с тем же числовым id.
CREATE TABLE IF NOT EXISTS media_items (
    id TEXT PRIMARY KEY,
    library_id INTEGER,
    title TEXT NOT NULL,
    year INTEGER,
    media_type TEXT NOT NULL, -- 'movie' или 'tv'
    file_id INTEGER,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(library_id) REFERENCES libraries(id) ON DELETE SET NULL,
    FOREIGN KEY(file_id) REFERENCES media_files(id) ON DELETE SET NULL
);

-- Эпизоды сериала: иерархия series -> (season_number) -> episode со ссылкой на файл.
CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY, -- UUID v7, единый namespace с media_items
    series_id TEXT NOT NULL,
    season_number INTEGER NOT NULL,
    episode_number INTEGER NOT NULL,
    title TEXT,
    file_id INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(series_id) REFERENCES media_items(id) ON DELETE CASCADE,
    FOREIGN KEY(file_id) REFERENCES media_files(id) ON DELETE CASCADE,
    UNIQUE(series_id, season_number, episode_number)
);

-- Прогресс просмотра — на уровне файла, чтобы работал и для фильмов, и для эпизодов.
CREATE TABLE IF NOT EXISTS playback_state (
    user_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    position INTEGER DEFAULT 0, -- секунда остановки
    completed BOOLEAN DEFAULT 0,
    last_watched DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(user_id, file_id),
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(file_id) REFERENCES media_files(id) ON DELETE CASCADE
);

-- Индексы для поиска
CREATE INDEX IF NOT EXISTS idx_media_items_title ON media_items(title);
CREATE INDEX IF NOT EXISTS idx_media_items_library_id ON media_items(library_id);
CREATE INDEX IF NOT EXISTS idx_media_items_media_type ON media_items(media_type);
CREATE INDEX IF NOT EXISTS idx_media_files_file_path ON media_files(file_path);
CREATE INDEX IF NOT EXISTS idx_episodes_series_id ON episodes(series_id);
CREATE INDEX IF NOT EXISTS idx_playback_state_user_id ON playback_state(user_id);
CREATE INDEX IF NOT EXISTS idx_playback_state_file_id ON playback_state(file_id);
