# План работ: переход на HLS VOD с картой сегментов

Рабочий документ на время рефакторинга. Архитектура и её обоснование — в [PLAYBACK.md](PLAYBACK.md),
здесь только пошаговое исполнение. Удалить после завершения и проверки на ТВ.

Порядок шагов важен: каждый следующий опирается на предыдущий. Компилировать после каждого шага.

---

## Шаг 0. Карта сегментов — СДЕЛАНО

Создан `src/segments.rs` со своими unit-тестами. Публичный API, на который опираются
остальные шаги:

```rust
pub const GRID_SEGMENT_SECONDS: f64 = 6.0;      // сетка при перекодировании видео

pub struct StreamProfile { pub transcode_video: bool, pub transcode_audio: bool }
impl StreamProfile { pub fn key(&self) -> &'static str }   // "copy" | "a" | "v" | "av"

pub struct SegmentMap { /* private */ }
impl SegmentMap {
    pub fn count(&self) -> usize;
    pub fn start_of(&self, index: usize) -> Option<f64>;
    pub fn duration_of(&self, index: usize) -> Option<f64>;
    pub fn to_json(&self) -> String;
    pub fn from_json(json: &str) -> Option<Self>;
    pub fn to_playlist(&self) -> String;
}

pub fn segment_name(index: usize) -> String;               // "segment00042.m4s"
pub fn parse_segment_name(name: &str) -> Option<usize>;    // защита от path traversal
pub async fn build_segment_map(file_path: &Path, duration: f64, profile: StreamProfile)
    -> Result<SegmentMap>;
```

**Ключевое свойство, на котором держится вся корректность.** Правило нарезки в карте
намеренно совпадает с алгоритмом HLS-муксера ffmpeg, поэтому границы в плейлисте и реальные
резы совпадают без дополнительной синхронизации:

| Профиль | Правило в `segments.rs` | Аргументы ffmpeg (шаг 2) |
|---|---|---|
| `-c:v copy` | первый keyframe, отстоящий от прошлой границы ≥ 4.0 с | `-hls_time 4` (муксер режет на первом keyframe после порога) |
| перекодирование видео | ровная сетка 6.0 с | `-hls_time 6 -force_key_frames expr:gte(t,n_forced*6)` |

Менять пороги можно только в обоих местах одновременно.

Осталось по этому шагу: зарегистрировать модуль — в `src/main.rs` добавить `mod segments;`
рядом с остальными (`mod scanner;` и т.д.).

---

## Шаг 1. Кэш карты в БД

### 1.1 Миграция `migrations/004_segment_index.sql`

```sql
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
```

### 1.2 Методы в `src/database.rs`

Рядом с `resolve_media_file` (около строки 337), в том же стиле (`sqlx::query`, `.context(...)`):

```rust
/// Возвращает закэшированную карту сегментов (JSON границ) или None.
pub async fn get_segment_map(&self, file_id: u64, profile: &str) -> Result<Option<String>>

/// Сохраняет карту сегментов; при повторе перезаписывает (INSERT OR REPLACE).
pub async fn save_segment_map(&self, file_id: u64, profile: &str, boundaries: &str) -> Result<()>
```

`file_id` биндить как `i64`. Тест в существующем `mod tests` файла: сохранить → прочитать →
сравнить; для несуществующей пары вернуть `None`.

---

## Шаг 2. `src/transcoder.rs`: раны вместо сессий-по-офсету

Полностью заменить `HlsSession` / `HlsManager` / `run_hls_ffmpeg` / `SessionKey` /
`HlsState` / `drop_other_offsets`. **`AudioTranscoder` и `KillOnDropChild` не трогать** —
`AudioTranscoder` ещё используется legacy-эндпоинтом `stream-transcoded`, `KillOnDropChild`
понадобится в новом коде.

### 2.1 Модель

Каталог кэша — на пару (файл, профиль), а не на офсет:
`std::env::temp_dir().join(format!("aurora-hls-{file_id}-{profile_key}"))`.
Он **не удаляется** при перемотке — только по TTL простоя. Именно это снимает гонку, из-за
которой второе устройство ломало первое.

Ран — процесс ffmpeg, стартующий с границы сегмента `start_index` и пишущий сегменты
`start_index, start_index+1, ...` в общий каталог. Ран не владеет URL: его смерть ничего не
инвалидирует, он только наполняет кэш.

```rust
/// Один запущенный ffmpeg, наполняющий кэш сегментов начиная с start_index.
struct Run {
    start_index: usize,
    /// Проверяется, чтобы не ждать вечно ран, который до нужного сегмента ещё не скоро дойдёт.
    task: tokio::task::JoinHandle<()>,
    failed: Arc<StdMutex<Option<String>>>,
}

/// Кэш сегментов одного файла в одном профиле.
struct FileCache {
    dir: PathBuf,
    profile: StreamProfile,
    runs: Vec<Run>,
    last_access: Instant,
}

#[derive(Clone)]
pub struct HlsManager {
    caches: Arc<AsyncMutex<HashMap<(u64, &'static str), Arc<AsyncMutex<FileCache>>>>>,
}
```

Внешняя `HashMap` под мьютексом только для поиска/создания записи; вся работа с конкретным
файлом — под его собственным мьютексом, чтобы разные фильмы не блокировали друг друга.

### 2.2 Публичный API

```rust
impl HlsManager {
    pub fn new() -> Self;

    /// Гарантирует наличие init.mp4 и возвращает его содержимое.
    pub async fn init_segment(
        &self, file_id: u64, file_path: &Path, profile: StreamProfile, map: &SegmentMap,
    ) -> Result<Vec<u8>>;

    /// Гарантирует наличие сегмента `index` и возвращает его содержимое.
    /// Либо отдаёт готовый файл, либо ждёт подходящий ран, либо запускает новый.
    pub async fn segment(
        &self, file_id: u64, file_path: &Path, profile: StreamProfile,
        map: &SegmentMap, index: usize,
    ) -> Result<Vec<u8>>;

    /// Удаляет кэши, простаивающие дольше ttl (глуша живые раны). Вызывается из main.rs.
    pub async fn cleanup_idle(&self, ttl: Duration);
}
```

`HlsManager` не знает про БД — карта передаётся аргументом. Сигнатура `cleanup_idle`
не меняется, `main.rs:85` править не нужно.

### 2.3 Алгоритм `segment(index)`

Константы: `LOOKAHEAD_SEGMENTS: usize = 6`, `MAX_RUNS_PER_FILE: usize = 2`,
`SEGMENT_WAIT_TIMEOUT: Duration = 30s`, `POLL_INTERVAL: Duration = 100ms`.

1. Путь `dir.join(segment_name(index))`. Файл есть → прочитать и вернуть.
2. Взять мьютекс `FileCache`, обновить `last_access`, перепроверить п.1.
3. Убрать из `runs` завершившиеся (`task.is_finished()`).
4. Найти ран, который до нужного сегмента дойдёт скоро:
   `r.start_index <= index && index <= frontier(dir, r.start_index) + LOOKAHEAD_SEGMENTS`,
   где `frontier` — наибольший `i`, для которого существуют все файлы от `r.start_index` до `i`
   (проверять `Path::exists`, начиная с `r.start_index`, до первого отсутствующего).
   Нашли → отпустить мьютекс и перейти к п.7.
5. Иначе запустить новый ран с `index`. Если `runs.len() >= MAX_RUNS_PER_FILE` — сначала
   заглушить самый старый (`task.abort()` + `Run` уже убивает процесс через `KillOnDropChild`).
   Каталог **не трогать**.
6. Добавить ран в `runs`, отпустить мьютекс.
7. Ждать: цикл до `SEGMENT_WAIT_TIMEOUT` с шагом `POLL_INTERVAL` — файл появился → вернуть;
   `failed` у рана заполнилось → вернуть ошибку с этим текстом. По таймауту — ошибка
   «сегмент не сгенерирован за N с».

`init_segment` — то же самое, но ждёт `init.mp4`; если ни одного рана нет, запускает ран с 0.

### 2.4 Команда ffmpeg

```rust
async fn spawn_run(
    file_path: &Path, dir: &Path, map: &SegmentMap,
    start_index: usize, profile: StreamProfile,
) -> Result<std::process::ExitStatus>
```

Аргументы по порядку:

```
-nostdin
-loglevel error
-ss <map.start_of(start_index)>        # только если > 0; быстрый seek до -i
-i <file_path>
-map 0:v:0
-map 0:a:0

# видео
если profile.transcode_video:
    -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p
    -force_key_frames expr:gte(t,n_forced*6)
иначе:
    -c:v copy

# аудио
если profile.transcode_audio:
    -c:a aac -b:a 192k -ac 2 -af aresample=async=1
иначе:
    -c:a copy

# таймстемпы — САМОЕ ВАЖНОЕ, см. предупреждение ниже
-copyts
-start_at_zero
-muxdelay 0

# сегментирование
-f hls
-hls_time <4 при copy | 6 при перекодировании видео>
-hls_list_size 0
-hls_flags temp_file
-hls_segment_type fmp4
-hls_fmp4_init_filename init.<start_index>.tmp.mp4
-hls_segment_filename <dir>/segment%05d.m4s
-start_number <start_index>
<dir>/run.<start_index>.m3u8
```

> **Предупреждение — не ставить `-avoid_negative_ts make_zero`.** В старой схеме он был уместен
> (каждая сессия имела собственный таймлайн с нуля), в новой он ломает всё: сегмент №100 начал
> бы объявлять себя нулевой секундой, и hls.js положил бы его в начало буфера. Абсолютные
> времена во всех ранах обеспечивают именно `-copyts -start_at_zero`.

Прочие обязательные детали:

- **`-hls_flags temp_file` оставить.** Он заставляет ffmpeg писать во временное имя и делать
  rename — читатель никогда не увидит недописанный сегмент. Без него ожидание из п.2.3
  вернёт обрезанный файл.
- **`-start_number <start_index>`** даёт глобальную нумерацию: файл на диске сразу называется
  так, как его назовёт `segment_name(index)` в плейлисте.
- **Плейлист ffmpeg (`run.<N>.m3u8`) не используется** — его пишет `to_playlist()` из карты.
  Файл нужен только потому, что муксер требует выходной путь; чистится вместе с каталогом.
- **`init.mp4` публикуется атомарно.** Каждый ран пишет свой `init.<N>.tmp.mp4`. Фоновая
  задача рана после появления временного файла делает `rename` в `init.mp4`, **только если
  `init.mp4` ещё нет** (иначе удаляет временный). Содержимое init одинаково между ранами
  (это конфигурация кодека, а не данные), поэтому первый победивший — правильный; важно лишь,
  чтобы клиент никогда не прочитал полузаписанный или подменённый файл.
- Процесс оборачивать в существующий `KillOnDropChild`, чтобы `task.abort()` действительно
  убивал ffmpeg, а не оставлял сироту.
- stderr читать и логировать как сейчас; при ненулевом коде возврата класть текст в
  `Run::failed`.

### 2.5 `cleanup_idle`

Пройти по кэшам, у которых `last_access.elapsed() > ttl`: заабортить все раны, удалить
каталог, убрать запись из `HashMap`. Логировать через `info!` как сейчас.

---

## Шаг 3. `src/server.rs`: роуты и хендлеры

### 3.1 Роуты

Заменить два роута с `:start` (строки 165–174) на:

```rust
.route("/api/media/:id/hls/playlist.m3u8", get(hls_playlist_handler))
.route("/api/media/:id/hls/:segment", get(hls_segment_handler))
```

Комментарий над ними обновить: относительные ссылки в плейлисте разрешаются в тот же
префикс, `:start` больше не нужен, потому что позиция — дело плеера.

### 3.2 Общий помощник

```rust
/// Резолвит файл, выводит профиль и достаёт карту сегментов — из кэша БД,
/// а при промахе строит через ffprobe и кэширует.
async fn segment_map_for(state: &AppState, id: &str)
    -> Result<(MediaFile, StreamProfile, SegmentMap), StatusCode>
```

Реализация:
1. `state.db.resolve_media_file(id)` → 404, если None.
2. `profile = StreamProfile { transcode_video: needs_video_transcoding(&f.video_codec, &f.pix_fmt),
   transcode_audio: needs_audio_transcoding(&f.audio_codec) }` — существующие функции не менять.
3. `state.db.get_segment_map(f.id, profile.key())` → есть и `SegmentMap::from_json` успешен →
   вернуть.
4. Иначе `build_segment_map(&f.file_path, f.duration? as f64, profile)`; при отсутствии
   `duration` вернуть 500 с понятным логом. Сохранить `save_segment_map`, вернуть.

### 3.3 `hls_playlist_handler`

Стал тривиальным: `Path(id)`, вызвать `segment_map_for`, вернуть `map.to_playlist()` с
`Content-Type: application/vnd.apple.mpegurl` и `Cache-Control: no-cache`.
**Никаких циклов ожидания и никакого 503** — весь блок с 50 итерациями по 200 мс удаляется.

### 3.4 `hls_segment_handler`

`Path((id, segment)): Path<(String, String)>`:
1. `segment == "init.mp4"` → `segment_map_for` → `state.hls.init_segment(...)`,
   `Content-Type: video/mp4`.
2. иначе `parse_segment_name(&segment)` → `None` даёт 400 (он же и есть защита от
   path traversal — старые проверки на `..` и слэши больше не нужны, но и не мешают);
   `index >= map.count()` → 404; иначе `state.hls.segment(...)`,
   `Content-Type: video/iso.segment`.
3. Ошибка менеджера → 500 с логом (не 404: 404 на сегмент снова заведёт клиент в ретраи).
4. `Cache-Control`: для сегментов теперь можно `public, max-age=31536000, immutable` —
   содержимое сегмента детерминировано номером. Для `init.mp4` — то же самое.
   Комментарий про «сессию можно пересоздать» удалить: он относился к старой модели.

### 3.5 Тесты в `server.rs`

Заменить `hls_playlist_with_offset_serves_segments` (строка ~1954) на два:

- `hls_playlist_is_vod_and_complete` — плейлист содержит `#EXT-X-PLAYLIST-TYPE:VOD`,
  `#EXT-X-ENDLIST`, `#EXT-X-MAP`, хотя бы один `#EXTINF`.
- `hls_segment_from_middle_served_first` — запросить сегмент из середины **первым**, до
  всякого обращения к нулевому: ожидается 200 и непустое тело. Это и есть проверка, что
  генерация ленивая и не зависит от порядка обращений (в старой схеме такой запрос давал 404).

Тестовый фикстур: там уже есть генерация файла для HLS-теста — переиспользовать её.
Если ffmpeg в CI нет, тест должен пропускаться так же, как это сделано сейчас.

---

## Шаг 4. Фронтенд: плеер

### 4.1 `frontend/src/api/client.ts`

`getHlsUrl(id: string, start = 0)` → `getHlsUrl(id: string)`; убрать сегмент `start` из пути:
`${this.baseUrl}/api/media/${id}/hls/playlist.m3u8`. Комментарий про офсет удалить.

### 4.2 `frontend/src/components/VideoPlayer.svelte`

Удалить:
- состояние `streamOffset` и derived `hlsSrc` (строки 55, 62);
- в `offsetActive` (строка 60) убрать `isHls`, оставить только `transcoded` — офсет теперь
  свойство исключительно legacy-потока;
- в `seekTo` (строки 274–287) весь блок `if (isHls)` со сравнением с `videoEl.seekable`;
- ручной seek по `startTime` в `handleLoadedMetadata` (строка 527) и в эффекте на строке 178
  для HLS-случая.

Изменить HLS-эффект (строка 65):
- источник — `src` напрямую (он больше не меняется за время просмотра), зависимость от
  позиции исчезает, эффект отрабатывает один раз на монтирование;
- конфиг: добавить `startPosition: startTime` рядом с существующими таймаутами. Это
  заменяет всю ручную установку начальной позиции;
- нативная ветка (webOS/Safari): после `loadedmetadata` выставить
  `video.currentTime = startTime` при `startTime > 0` — там `startPosition` не работает;
- обработчик `ERROR`: завести счётчик `fatalCount`. При фатальной ошибке инкремент; если
  `fatalCount > 3` — `hls.destroy()` и показать сообщение (новое состояние
  `playbackError = $state<string | null>(null)`, отрисовать поверх видео). Иначе прежние
  `startLoad()` / `recoverMediaError()`, но с задержкой `500 * fatalCount` мс.

`seekTo` для HLS сводится к общей ветке: `videoEl.currentTime = target`.

### 4.3 Проверить, что стало проще

После правок в плеере не должно остаться ни одного места, где к `videoEl.currentTime`
прибавляется или вычитается что-либо для HLS-потока. `absoluteTime()` для HLS обязан
возвращать ровно `videoEl.currentTime`.

---

## Шаг 5. Прогресс и мультиустройство

### 5.1 Единственный владелец

В `frontend/src/routes/Watch.svelte` из `handleTimeUpdate` (строка 91) удалить вызов
`apiClient.updateProgress` и переменную `lastSave`; оставить только обновление UI, если оно
где-то нужно (иначе проп `onTimeUpdate` можно не передавать вовсе). Сохраняет только
`VideoPlayer`.

### 5.2 Принудительный флаш

В `VideoPlayer.svelte` завести `flushProgress()` — синхронно берёт `absoluteTime()` и зовёт
`saveProgress`. Вызывать из:
- существующего интервала (оставить как есть);
- `onpause` у `<video>`;
- `visibilitychange`, когда `document.hidden`;
- функции очистки эффекта размонтирования (строка 187) и `pagehide`.

Для `visibilitychange` / `pagehide` использовать `navigator.sendBeacon`, если доступен, —
обычный `fetch`/axios при выгрузке страницы может не успеть уйти. Иначе — обычный вызов.

### 5.3 Автопауза при потере видимости

В том же обработчике `visibilitychange → document.hidden`: `videoEl.pause()`.
Это решает конфликт «забытый включённым ТВ перетирает позицию планшета» без серверного
состояния и попутно освобождает CPU сервера. При возврате видимости **не** возобновлять
автоматически.

### 5.4 Флаг completed

Эффект «90%» (строка 224) сейчас дёргает `saveProgress` на каждом тике после порога.
Заменить на `let completedSent = false;` — отправлять `completed: true` один раз.

### 5.5 Кэш прогресса

Убедиться, что `GET /api/media/:id/progress` отдаётся без HTTP-кэша (при необходимости
добавить `Cache-Control: no-store` в `get_progress_handler`), иначе устройство с давно
открытой вкладкой стартует со старой позиции.

---

## Шаг 6. Проверка

### Автоматическая

```
cargo fmt && cargo clippy && cargo test
cd frontend && npm run build
```

Все тесты `segments.rs` должны проходить без ffmpeg (они чисто вычислительные).

### Ручная — матрица файлов

Три файла, покрывающие все ветки профиля:

| Файл | Профиль | Что проверяет |
|---|---|---|
| h264 + aac | `copy` | границы по keyframes, `-c:v copy -c:a copy` |
| h264 + ac3 | `a` | транскодинг аудио, синхронизация с видео |
| hevc 10-bit | `av` или `v` | сетка 6 с, `-force_key_frames` |

Для каждого — пять сценариев:
1. Старт с нуля.
2. Перезагрузка страницы → продолжение с сохранённой позиции.
3. Перемотка вперёд на 30 минут.
4. Перемотка назад в уже просмотренное (обязана отдаваться из кэша, мгновенно).
5. Серия быстрых перемоток подряд.

Критерий приёмки: **ни одного повторного проигрывания отрезка и ни одного зависания**;
в консоли нет фатальных ошибок hls.js; в логах сервера нет 404/503 на сегменты.

### Ручная — мультиустройство

- Запустить фильм в одном браузере, посмотреть 5 минут, свернуть вкладку. Проверить, что
  воспроизведение встало на паузу и позиция записалась.
- Открыть тот же фильм в другом браузере под тем же пользователем: стартует с той же секунды
  и **без запуска нового ffmpeg** — сегменты берутся из кэша. Проверять по логам сервера и
  `pgrep -fa ffmpeg`.
- Вернуться обратно.
- Одновременное воспроизведение с двух устройств не должно ломать ни одно из них.

### Ресурсы

`pgrep -fa ffmpeg` после серии перемоток: живых процессов не больше `MAX_RUNS_PER_FILE` на
файл, после ухода со страницы они догорают сами (ран доходит до конца или глушится по TTL).

---

## Шаг 7. Ревью реализации (найденные расхождения)

Шаги 0–5 выполнены. Ниже — то, что при ревью кода не сошлось с планом или сломано.
Порядок — по убыванию важности; блокеры (1–4) закрыть до ручной проверки на ТВ.

### Блокеры

- [x] **7.1 `cargo test` красный.** `segments::tests::keyframes_collapse_into_segments_of_at_least_four_seconds`
      падает: при GOP 2 с на 30-секундном файле последний сегмент 28→30 = 2 с, а
      `MIN_TAIL_SECONDS = 1.0` его не сливает. Решение: либо поднять порог слияния хвоста до
      `MIN_COPY_SEGMENT_SECONDS`, либо ослабить инвариант теста (последний сегмент — исключение).

- [x] **7.2 `-force_key_frames` ломается на ранах не с нуля** (`transcoder.rs:468`).
      С `-copyts` `t` — абсолютное время файла, а `n_forced` стартует с 0, поэтому для рана
      с 1200-й секунды `gte(t, n_forced*6)` истинно сразу и ffmpeg форсит I-кадр на каждом кадре,
      пока `n_forced*6` не догонит 1200 (~200 keyframes подряд). Нужно смещение:
      `expr:gte(t,<start_time>+n_forced*6)`.

- [x] **7.3 Хит из кэша не обновляет `last_access`** (`transcoder.rs:327`): быстрый путь
      `fs::read` возвращает данные до взятия мьютекса. Зритель, 15 минут смотрящий уже
      сгенерированный участок (перемотка назад — сценарий 4 из матрицы), попадает под
      `cleanup_idle` и теряет каталог под собой. Обновлять `last_access` и на быстром пути.

- [x] **7.4 `publish_init` может опубликовать недописанный init.** `-hls_flags temp_file`
      защищает только сегменты; `init.<N>.tmp.mp4` пишется напрямую, а `publish_init`
      (`transcoder.rs:568-585`) делает `rename` сразу, как файл появился. При гонке или
      `task.abort()` в момент записи в `init.mp4` уедет обрезанный moov — и это не чинится,
      файл живёт до TTL. Ждать не `exists`, а появления первого сегмента этого рана.

### Расхождения с планом

- [ ] **7.5 Ручной seek в `handleLoadedMetadata` не убран** (`VideoPlayer.svelte`, `handleLoadedMetadata`), хотя
      п. 4.2 требовал его снять в пользу `startPosition`. Срабатывает на каждом
      `loadedmetadata`, включая повторные после `recoverMediaError()`, и отбрасывает playhead
      на `startTime` — тот самый прыжок назад, ради устранения которого делался рефакторинг.
      Плюс дублирующий эффект на строках 218–224. Оставить только нативную ветку
      (webOS/Safari), ограничив её случаем `!Hls.isSupported()`.

- [ ] **7.6 Хвост карты не совпадает с нарезкой ffmpeg.** `from_boundaries` сливает хвост < 1 с
      с предыдущим сегментом, муксер — нет: он выпустит лишний файл с номером `count()`,
      которого нет в плейлисте, а последний `#EXTINF` окажется длиннее реального содержимого.
      Плюс `duration` в БД целочисленная. Либо не сливать хвост, либо считать последний
      `EXTINF` по факту.

- [ ] **7.7 `frontier()` — линейный `Path::exists` от `start_index` под мьютексом**
      (`transcoder.rs:227`). Если ран ушёл на 2000 сегментов вперёд, каждый запрос делает
      ~2000 stat-вызовов на каждый ран, блокируя `FileCache`. Кэшировать достигнутую границу
      в `Run` (она монотонно растёт).

- [ ] **7.8 `MAX_RUNS_PER_FILE = 2` — общий на файл, а не на зрителя.** Два устройства с
      перемотками вытесняют раны друг друга: URL не ломаются (в этом смысл новой модели), но
      возможны взаимные подвисания до `SEGMENT_WAIT_TIMEOUT`. Ограничение логичнее вешать на
      общее число ffmpeg, а вытеснять самый дальний от запроса ран, а не `runs.remove(0)`.

- [ ] **7.9 Карта в БД не инвалидируется при подмене файла.** `PRIMARY KEY (file_id, profile)`:
      пересканирование того же пути с другим содержимым отдаст старые границы. Завязать ключ
      или проверку на `size`/`mtime`.

- [ ] **7.10 `cleanup_idle` держит внешний мьютекс `caches`, пока берёт внутренние**
      (`transcoder.rs:413-431`) — на время уборки запросы к любому файлу встают в очередь на
      `cache_for`. Собрать ключи, отпустить внешний лок, потом чистить.

### Мелочи

- [ ] **7.11** `cleanup_idle` удаляет каталог, но `Arc<FileCache>`, уже взятый параллельным
      запросом, продолжает указывать на удалённый `dir`, а тот не пересоздаётся (создание
      только в `or_insert_with`). Добавить `create_dir_all` перед стартом рана.
- [ ] **7.12** `hlsSrc` (`VideoPlayer.svelte`) игнорирует проп `src` и пересобирает URL из
      `mediaId` — `Watch.svelte` передаёт ровно то же самое, дублирование можно убрать.
      `streamOffset` остался только ради legacy-потока — это осознанно.

---

## После завершения

- Удалить этот файл.
- В `ROADMAP.md` пункт 1.2 привести в соответствие: сейчас там написано
  `-hls_playlist_type vod` и «ffmpeg дожидается полного завершения перед отдачей плейлиста»,
  что не соответствовало коду уже до этого рефакторинга. Заменить на ссылку на `PLAYBACK.md`.
- Подпункт 1.2 «Удалить `/api/media/:id/stream-transcoded`» остаётся невыполненным осознанно.
