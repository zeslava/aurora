-- Название из TMDB хранится отдельно от имени файла и ручной правки.
ALTER TABLE media_items ADD COLUMN tmdb_title TEXT;
-- Язык последней попытки: смена TMDB_LANGUAGE возвращает элемент в очередь.
ALTER TABLE media_items ADD COLUMN tmdb_language TEXT;
