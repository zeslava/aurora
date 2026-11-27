-- Год TMDB хранится отдельно: повторный скан меняет только год из имени файла.
ALTER TABLE media_items ADD COLUMN tmdb_year INTEGER;

-- Ранее найденные записи без года один раз возвращаются в очередь за датой премьеры.
UPDATE media_items SET tmdb_synced_at = NULL
WHERE year IS NULL AND tmdb_id IS NOT NULL;
