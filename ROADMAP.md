# Aurora — план улучшений

Фокус: UX и простота использования, в первую очередь на телевизоре (LG webOS).
Порядок: фундамент, без которого ТВ-опыт не собрать (модель данных, HLS) → ТВ-интерфейс → удобство → безопасность → качество кода.

## 1. Фундамент для ТВ-опыта

Без этого «продолжить просмотр», следующий эпизод и нормальная перемотка невозможны.

- [x] **1.1 Модель данных.** Сейчас эпизоды лежат как отдельные `MediaItem`, таблица `episodes` пустая, ID пересекаются. Нужно: `media_files` (файл, кодеки, длительность) + иерархия `series → seasons → episodes` со ссылками на файлы. Прогресс просмотра — на уровне файла, чтобы работал и для эпизодов
  - [x] Схема в `001_initial_schema.sql`: `media_files(id, file_path, duration, resolution, video_codec, audio_codec, video_bitrate)`
  - [x] `episodes(id, series_id, season_number, episode_number, title, file_id → media_files)` — без отдельной таблицы `seasons`, номер сезона хранится прямо в `episodes` (сознательное упрощение, отдельной сущности `Season` не заводили)
  - [x] `playback_state` уже на `file_id`, а не `media_id`
  - [x] Проекта с нуля — таблица `episodes` не была пустой исторически, миграции данных из старой схемы не требовалось
  - [x] `models.rs`: типы `MediaFile`, `Episode` (с `file_id`) на месте
  - [x] `database.rs`: CRUD под новую схему (`insert_media_file`, `insert_episode`, `resolve_media_file`, `upsert_playback_state` по `file_id` и т.д.)
  - [x] `scanner.rs` + `server.rs`: скан пишет в `media_files`, строит иерархию series→episodes через `insert_media_item`/`insert_episode`
- [x] **1.2 HLS вместо самодельного chunked-транскодинга** (`ffmpeg -f hls`). Для ТВ это обязательно: webOS нативно играет HLS через `<video>`, даёт точную перемотку и известную длительность — ровно то, чего не хватает текущей схеме «2-минутные чанки + байты→секунды»
  - [x] `transcoder.rs`: `HlsManager`/`HlsSession` — VOD-плейлист через `ffmpeg -f hls -hls_playlist_type vod -hls_time 6 -hls_segment_type fmp4 -c:v copy` (аудио → AAC при несовместимом кодеке: ac3/dts/truehd/eac3). Сегменты + `playlist.m3u8` в temp dir на сессию; ffmpeg-процесс дожидается полного завершения перед отдачей плейлиста (упрощение: сразу полный VOD с `EXT-X-ENDLIST`, без прогрессивной отдачи)
  - [x] Эндпоинты в `server.rs`: `GET /api/media/:id/hls/playlist.m3u8` и `/api/media/:id/hls/:segment` (резолвят `:id` как эпизод или media item через `resolve_media_file`, как в `stream_transcoded_handler`); защита от path traversal в имени сегмента
  - [x] Сессия: ключ по `file_id` в `HlsManager` (поле `AppState.hls`), temp dir в `std::env::temp_dir()/aurora-hls-<file_id>`, фоновая TTL-очистка простаивающих сессий каждые 5 мин (порог простоя 15 мин) в `main.rs`; повторные запросы переиспользуют готовую сессию
  - [x] Фронтенд `VideoPlayer.svelte`: hls.js (`Hls.isSupported()`), fallback на нативный `<video src>` (`canPlayType('application/vnd.apple.mpegurl')`) для Safari/webOS; `Watch.svelte` переключён на `apiClient.getHlsUrl()`
  - [ ] Удалить `/api/media/:id/stream-transcoded` и логику «2-мин чанки + байты→секунды» — оставлено как есть до полной проверки HLS-пути в проде/на ТВ
  - [x] **Переработка HLS: сессии-по-офсету → карта сегментов (VOD).** Схема «`(file_id, start)` + `EVENT`-плейлист» давала зависания и повторное проигрывание отрезка (плеер считал поток live и сам двигал позицию). Реализовано: `segments.rs` (`SegmentMap`), кэш карты в `media_file_segments` (миграция `004_segment_index.sql`), VOD-плейлист на весь файл + ленивая генерация сегментов в `transcoder.rs`. Архитектура и разбор причин — [PLAYBACK.md](PLAYBACK.md), план работ и открытые пункты ревью (7.5–7.12) — [PLAYBACK_PLAN.md](PLAYBACK_PLAN.md)
- [x] **1.3 Транскодинг по видеокодеку** (webOS-браузер не играет HEVC/10-bit в вебе — детектить и перекодировать/ремуксить)
  - [x] `metadata.rs`: вернуть `pix_fmt` из ffprobe (миграция `002_add_pix_fmt.sql`, колонка `media_files.pix_fmt`); считать несовместимым `hevc`/`h265`, `vp9` и 10-bit (`yuv420p10le`/`10be`)
  - [x] Совместимый видеокодек (h264 8-bit) → ремукс `-c:v copy`; несовместимый → `-c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p` в том же HLS-pipeline (1.2)
  - [x] Правило выбора вынесено в `needs_video_transcoding` (`server.rs`), покрыто unit-тестами
- [x] **1.4 Фоновый скан** (`202` + job id) и `spawn_blocking` — чтобы UI не зависал при обновлении библиотеки
  - [x] `POST /api/scan` → `202 Accepted` + `{ "job_id": "<uuid>" }`; сам скан в `tokio::task::spawn_blocking` (walkdir блокирующий; ffprobe в `insert_file_if_new` остаётся синхронным вызовом внутри async-задачи — не блокирует другие handlers, т.к. вся работа джоба идёт в отдельной `tokio::spawn`-задаче)
  - [x] `ScanJobs` в `AppState`: `Arc<RwLock<HashMap<Uuid, JobStatus>>>`, `JobStatus { state: Running|Done|Failed, scanned, total, error }`
  - [x] `GET /api/scan/:job_id` → текущий `JobStatus` (404 если нет)

## 2. ТВ-интерфейс (LG webOS — приоритет)

10-foot UI: управление пультом, крупные элементы, минимум текста.

- [x] **2.1 Навигация D-pad'ом**: работа со стрелок + OK + Back, видимый фокус на каждом фокусируемом элементе. `initSpatialNavigation()` (перехват `keydown` 37–40/13/461-Back), выбор ближайшего элемента по геометрии `getBoundingClientRect`; стили фокуса через `:focus-visible`. Все карточки/кнопки — фокусируемые (`tabindex=0`)
  - [x] `lib/spatialNavigation.ts`: скоринг `along + across * 3` по центрам `getBoundingClientRect`, OK (13) кликает не-кнопки, Back (461/10009/Escape) → `history.back()`
  - [x] Подключён в `App.svelte` через `$effect` (функция возвращает cleanup) — один глобальный слушатель на всё приложение
  - [x] `:focus-visible` в `index.css` + `@supports not selector(:focus-visible)` фолбэк для старых webOS
  - [x] `MediaCard.svelte` — `tabindex=0`, оверлей по `:focus-within`; внутренняя кнопка `tabindex={-1}`, чтобы не дублировать цель фокуса
  - [x] `VideoPlayer.svelte` помечен `data-no-spatial` — стрелки внутри плеера не перехватываются (свои хоткеи в 2.5)
- [x] **2.2 Magic Remote** (курсор): не ломать hover/click при курсорном режиме, работать параллельно с 2.1 (курсор двигает фокус, стрелки — тоже)
  - [x] `lib/pointerMode.ts`: `mousemove` → pointer-режим + фокус на элемент под курсором (троттлинг `requestAnimationFrame`, `focus({preventScroll})`); D-pad-клавиши (13/37-40/461/10009) → key-режим; режим в `data-input-mode` на `<html>`
  - [x] Подключён в `App.svelte` через `$effect` параллельно с `initSpatialNavigation`
  - [x] `index.css`: в `[data-input-mode='pointer']` гасим D-pad-обводку (`:focus`/`:focus-visible`), hover/click остаются; OK Magic Remote — нативный клик под курсором
- [x] **2.3 Вход без клавиатуры**: pairing-код — на ТВ показывается 6-значный код, подтверждение с телефона/десктопа (аналог Jellyfin Quick Connect). Набор пароля пультом — худший сценарий
  - [x] `POST /api/pair/start` → `{ code, pairing_id }` (код 6 цифр, TTL 5 мин, in-memory `PairingManager` в `AppState`, фоновая очистка истёкших сессий раз в минуту в `main.rs`)
  - [x] `POST /api/pair/confirm` (требует JWT авторизованного устройства) — привязать код к пользователю
  - [x] `POST /api/pair/poll` (ТВ опрашивает по `pairing_id`) → `{ token }` после подтверждения (сессия удаляется), иначе `pending`, 404 если истёк
  - [x] Фронтенд: вкладка «ТВ» на `Login.svelte` (`TvPairing.svelte`: показ кода + polling каждые 3 сек), модалка «Подключить ТВ» (`PairTvModal.svelte`) для ввода кода авторизованным устройством
- [x] **2.4 Главный экран** = «Продолжить просмотр» + «Недавно добавленное», а не плоский список файлов
  - [x] `GET /api/home` (требует JWT) → `{ continue_watching: [...], recently_added: [...] }`. Continue: `playback_state` пользователя, где `completed = 0` и `0 < position < duration`, отдельными запросами по фильмам и эпизодам, слияние и сортировка по `last_watched DESC` (лимит 20); recently: `media_items` по `created_at DESC` (лимит 20). Тип `ContinueWatchingItem` в `models.rs`, методы `get_continue_watching`/`get_recently_added` в `database.rs`
  - [x] Фронтенд: секции «Продолжить просмотр» (горизонтальный ряд `ContinueCard.svelte`, навигация на `/watch/...`) и «Недавно добавленное» (сетка `MediaCard.svelte`) сверху `Library.svelte`, показываются при пустом поиске и фильтре «Все»; ниже — «Вся медиатека» со старым списком/фильтрами
- [x] **2.5 Плеер под пульт**: в `VideoPlayer.svelte` — OK toggle play/pause, ←/→ seek ±10 сек, стрелки показывают overlay управления. Автопереход: за N сек до конца — оверлей «Следующий эпизод» с обратным отсчётом (нужен «next episode» из иерархии 1.1), OK/таймаут → переход
  - [x] `keydown`: `Enter` (OK пульта) добавлен к `Space` → toggle play/pause; если показан оверлей «Следующий эпизод» — OK переходит к нему. Стрелки ←/→/↑/↓ вызывают `resetControlsTimeout()` → показывают контролы
  - [x] Оверлей «Следующий эпизод»: за `NEXT_EP_THRESHOLD` (15 сек) до конца при `isPlaying`, кнопка с обратным отсчётом (`Math.ceil(remaining)`), фокусируемая (клик Magic Remote / OK). Таймаут → `onEnded` → тот же переход
  - [x] `Watch.svelte`: логика следующего эпизода вынесена в `nextEpisodePath()`/`goToNextEpisode()` (эпизод+1 или первый эпизод следующего сезона), передаётся в плеер как `onNextEpisode` (undefined для фильма/последнего эпизода)
- [x] **2.6 Упаковка в webOS-приложение** (ares-cli): `appinfo.json` + иконки, `ares-package` из собранного `frontend/dist`; приложение указывает на URL сервера. Документировать сборку `.ipk`
  - [x] `frontend/webos/`: `appinfo.json` (id `com.aurora.mediaserver`, type `web`, main `index.html`) + `icon.png` 80×80 / `largeIcon.png` 130×130 (плейсхолдеры)
  - [x] Адрес сервера вводится в самом приложении при первом запуске (экран `routes/ServerSetup.svelte`, гейт в `App.svelte` по `apiClient.needsServerSetup()` при запуске не по http(s)); хранится в `localStorage`, приоритет над сборочным `VITE_API_URL` — один `.ipk` работает с любым сервером. `client.ts`: `getServerUrl`/`setServerUrl`, динамический `baseUrl` для api/stream/HLS
  - [x] `vite.config.ts`: `base` из `VITE_BASE` (webOS-сборка ставит `./` для относительных путей к ассетам — index.html грузится по `file://`; веб-деплой остаётся `/`)
  - [x] `scripts/build-webos.sh`: сборка фронта (`VITE_BASE=./`, опц. `VITE_API_URL`) → staging (`dist` + иконки + `appinfo.json` с version из `package.json`) → `ares-package` → `build/ipk/*.ipk`
  - [x] Документация в `README.md`: сборка `.ipk`, sideload через Developer Mode (`ares-setup-device`/`ares-install`/`ares-launch`), артефакт в GitHub Releases
- [ ] **2.7 Проверка на реальном ТВ**: старые webOS (2020−, Chromium ~68) — проверить целевой browserslist в Vite, добавить полифиллы при необходимости; не полагаться на свежие CSS/JS-фичи

## 3. Простота использования

- [ ] **3.1 Zero-config старт**: если в БД нет пользователей — API отдаёт «setup required», фронт показывает мастер (создать админа + указать `MEDIA_PATHS`). Пути библиотек сохранять в БД (таблица `libraries`), а не только из env. `POST /api/setup` доступен только при пустой таблице users
- [ ] **3.2 Авто-скан**: `notify`-watcher на путях библиотек (debounce ~2 сек на всплеск событий) → инкрементальный скан изменившихся путей; плюс периодический полный скан (интервал из конфига). Переиспользовать job-механику 1.4. Кнопка «Scan» становится опциональной
- [x] **3.3 Постеры и превью**: сделано иначе, чем планировалось — вместо кадра из ffmpeg сразу пошли в TMDB, кадр не понадобился
  - [x] `tmdb.rs`: поиск по названию+году (`search/movie`, `search/tv`), загрузка постеров/бэкдропов в `<CACHE_DIR>/images`, миграции `003_tmdb_metadata.sql` / `007_season_posters.sql`. Обогащение — фоном после каждого скана и на старте для `tmdb_synced_at IS NULL`; ручной перезапуск — `POST /api/metadata/refresh`
  - [x] `GET /api/media/:id/poster`, `/api/media/:id/backdrop`, `/api/series/:id/seasons/:season/poster` (публичные, как стримы — `<img>` не шлёт заголовок авторизации)
  - [x] Сетка постеров на фронте (`Library.svelte`, `MediaCard.svelte`/`Poster.svelte`); без `TMDB_API_KEY` — генерируемый `PosterPlaceholder.svelte`
  - [ ] Кадр из файла (`ffmpeg -ss <10%> … -frames:v 1`) как фолбэк, когда TMDB не нашёл совпадения
- [x] **3.4 Человеческие названия**: парсер имени файла → title + year («The.Matrix.1999.1080p» → «Матрица (1999)»). Regex в `scanner.rs` (см. 5.4): вырезан год `(19|20)\d\d`, release-теги (качество/кодек/группа), точки/подчёркивания → пробелы; название уточняется из TMDB (3.3). Ручное переименование — `PATCH /api/media/:id` + `custom_title` (миграция `006_custom_title.sql`, `TitleEditor.svelte`)
- [ ] **3.5 Обнаружение удалённых файлов**: при скане помечать записи, чьих файлов нет на диске (`Path::exists`) — soft-delete/флаг `missing`, скрывать в выдаче; чистить осиротевший `playback_state`
- [ ] **3.6 Поиск** (с ТВ — по буквам через экранную мини-клавиатуру, максимально простую)
  - [ ] `GET /api/search?q=` — `LIKE %q%` по `title` (media_items + episodes), лимит
  - [ ] Экранная мини-клавиатура на ТВ (QWERTY/раскладка), навигация 2.1, ввод по OK

## 4. Безопасность (минимум, не мешающий простоте)

- [ ] **4.1** Подключить `auth_middleware` через `axum::middleware::from_fn` на `/api/*`, кроме `login`/`setup`/`pair`/stream+hls+poster. Для stream/HLS — короткоживущий (~1 ч) query-токен, чтобы работало в `<video src>`; выдавать отдельным эндпоинтом, проверять в хендлере
- [ ] **4.2** `JWT_SECRET`: при старте, если не задан — сгенерировать (32 байта, base64) и сохранить (в БД `settings` или файл), а не хардкод дев-ключ. Логировать факт генерации
- [ ] **4.3** Регистрация: первый пользователь (пустая таблица) → роль `admin` (см. 3.1); дальше регистрация только по инвайт-токену. Поле `role` в `users`, таблица/поле инвайтов
- [ ] **4.4** `stream_handler`: `file_size == 0` → `500`/`422` вместо panic (сейчас деление/паника); невалидный или out-of-range `Range` → `416 Range Not Satisfiable` с `Content-Range: bytes */<size>`
- [ ] **4.5** Глобальный `tokio::sync::Semaphore` в `AppState` на число ffmpeg-процессов (лимит из конфига, дефолт ~2–3); транскодер берёт permit на время сессии, при исчерпании — `503`

## 5. Качество кода

- [x] **5.1** `sqlx::migrate!("./migrations")` (встраивает миграции в бинарь) вместо чтения `./migrations` по относительному пути в рантайме — `database.rs::run_migrations`
- [ ] **5.2** `#[derive(sqlx::FromRow)]` на доменных типах в `models.rs` + `query_as::<_, T>()` — убрать ручной маппинг строк в `database.rs`
- [ ] **5.3** `ApiError` (`thiserror`) + `impl IntoResponse` (маппинг вариантов → HTTP-код + JSON); хендлеры возвращают `Result<_, ApiError>`, убрать повторяющиеся `(StatusCode::INTERNAL_SERVER_ERROR, Json(json!(...)))`
- [ ] **5.4** Скомпилированные regex в `scanner.rs` → `static ... = LazyLock::new(|| Regex::new(...))` вместо компиляции на каждый файл
- [ ] **5.5** Разбить `server.rs`: выделить `streaming` (stream/HLS/range) и `scan` в отдельные модули; в `server.rs` оставить сборку роутера и общие хендлеры

## Порядок реализации

Разделы 1 и 2 закрыты (кроме 1.2 «удалить `stream-transcoded`» и 2.7 «проверка на реальном ТВ»),
в разделе 3 сделаны 3.3 и 3.4. Что осталось:

- **Критический путь:** 3.5 (битые файлы) → 3.1 (zero-config) → 3.2 (авто-скан) → 3.6 (поиск)
- **Можно начинать сразу параллельно:** 4.2, 4.4, 4.5, 5.4, 5.5
- **Отдельным треком:** открытые пункты ревью плеера 7.5–7.12 в [PLAYBACK_PLAN.md](PLAYBACK_PLAN.md)

## Что сознательно не делаем (простота важнее)

- Мультисерверность, федерация, плагины
- Транскодинг «на все случаи» — только то, что нужно webOS и десктопным браузерам
- Настройки качества у пользователя — сервер сам решает, нужен ли транскодинг
