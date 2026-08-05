-- Карта границ сегментов HLS: чистая функция от файла и профиля потока,
-- строится ffprobe'ом один раз и переиспользуется всеми клиентами.
CREATE TABLE IF NOT EXISTS media_file_segments (
    file_id     INTEGER NOT NULL,
    profile     TEXT    NOT NULL,
    boundaries  TEXT    NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (file_id, profile),
    FOREIGN KEY (file_id) REFERENCES media_files(id) ON DELETE CASCADE
);
