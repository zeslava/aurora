-- Постеры сезонов сериала. Сезон не сущность в схеме (эпизоды хранят номер),
-- поэтому обложка привязана к паре (сериал, номер сезона).
-- poster_file — имя файла в CACHE_DIR/images, как у media_items.poster_file.
CREATE TABLE IF NOT EXISTS season_images (
    series_id TEXT NOT NULL,
    season_number INTEGER NOT NULL,
    poster_file TEXT,
    PRIMARY KEY (series_id, season_number),
    FOREIGN KEY(series_id) REFERENCES media_items(id) ON DELETE CASCADE
);
