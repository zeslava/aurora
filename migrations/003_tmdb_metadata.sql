-- Метаданные TMDB для фильмов и сериалов.
-- poster_file/backdrop_file — имена файлов в CACHE_DIR/images (изображения кэшируются
-- на диск, чтобы клиент не ходил в интернет: важно для webOS и офлайн-сети).
ALTER TABLE media_items ADD COLUMN tmdb_id INTEGER;
ALTER TABLE media_items ADD COLUMN overview TEXT;
ALTER TABLE media_items ADD COLUMN rating REAL;
ALTER TABLE media_items ADD COLUMN poster_file TEXT;
ALTER TABLE media_items ADD COLUMN backdrop_file TEXT;
-- Отметка о попытке синхронизации: NULL = ещё не пробовали (в очереди на догрузку)
ALTER TABLE media_items ADD COLUMN tmdb_synced_at DATETIME;
