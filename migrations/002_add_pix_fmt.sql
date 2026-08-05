-- Пиксельный формат видео (нужен для решения copy|transcode в HLS: hevc/vp9/10-bit не играются в браузере)
ALTER TABLE media_files ADD COLUMN pix_fmt TEXT;
