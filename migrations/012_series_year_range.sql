-- Год окончания хранится только для завершённых сериалов; NULL в ongoing — статус неизвестен.
ALTER TABLE media_items ADD COLUMN tmdb_end_year INTEGER;
ALTER TABLE media_items ADD COLUMN tmdb_ongoing BOOLEAN;

-- Догружаем диапазон у сериалов, уже найденных до появления этих полей.
UPDATE media_items SET tmdb_synced_at = NULL
WHERE media_type = 'tv' AND tmdb_id IS NOT NULL;
