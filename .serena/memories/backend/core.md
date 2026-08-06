# Бэкенд — карта

`main.rs` собирает `AppState { config, db, metrics }` → axum. Плоский набор модулей в `src/`:

- `server.rs` (~3k строк) — **все** роуты и хендлеры. API под `/api/*`, всё остальное — фолбэк на SPA (`index.html`). Любое изменение эндпоинта — сюда.
- `config.rs` — чтение env (см. ниже).
- `database.rs` — доступ к данным через sqlx/SQLite; миграции из `migrations/` на старте.
- `models.rs` — доменные типы (`MediaItem`, `Library`, `MediaType`, `PlaybackState`, эпизоды, метаданные).
- `scanner.rs` — обход `MEDIA_PATHS` (walkdir) и разбор имён. Фильм vs сериал определяется **по содержимому/паттернам имён, не по имени каталога**: `SxxEyy` → эпизод, группы эпизодов сливаются по общему префиксу (`merge_by_common_prefix`), ≥2 файла `part N` с общим каталогом/названием/годом → один сериал (`split_multipart_movies`); остальное — фильм.
- `metadata.rs` — разбор JSON от `ffmpeg::probe_format_json` (длительность, разрешение, кодеки, битрейт).
- `auth.rs` — JWT (jsonwebtoken, 7 дней) + bcrypt; токен из заголовка `Authorization`.
- `pairing.rs` — привязка TV-устройства (парный код), UI — `PairTvModal`/`TvPairing`.
- `tmdb.rs` — поиск по названию + скачивание постеров/бэкдропов в `<CACHE_DIR>/images`. Обогащение — фоновой задачей после каждого скана и на старте для `tmdb_synced_at IS NULL`. Картинки отдаются публично через `/api/media/:id/poster|backdrop` (у `<img>` нет заголовка Authorization).
- `metrics.rs` — `MetricsCollector` (sysinfo), лог раз в 60 с, `/api/metrics`. Плюс `SEGMENT_DRIFT` — счётчик инварианта «сегмент длится ровно столько, сколько обещает карта»; `mismatched > 0` — баг, даже без жалоб.
- `static_files.rs` — отдача SPA: с диска, если существует `STATIC_DIR`, иначе из вшитого rust-embed.

Стриминг (`ffmpeg.rs`, `transcoder.rs`, `segments.rs`, `testkit.rs`) и его тесты — отдельно: `mem:backend/streaming`.

## Данные
Таблицы: `users`, `libraries`, `media_items`, `episodes` (принадлежат сериалу-`media_item`), `playback_state` (позиция на пару user+media). Базовая схема — `migrations/001_initial_schema.sql`, изменения — только новой миграцией.

## Env
`PORT` (4000), `DATABASE_URL`/`DB_PATH` (`sqlite:./aurora.db`, локально `./data/aurora.db`), `MEDIA_PATHS` (через запятую; одного корня достаточно), `STATIC_DIR` (dev-оверрайд), `JWT_SECRET` (**дефолт — хардкод, обязателен в проде**), `LOG_LEVEL`, `TMDB_API_KEY` (без него обложки отключены, UI рисует плейсхолдеры), `TMDB_LANGUAGE` (`ru-RU`), `CACHE_DIR` (`./cache`).
