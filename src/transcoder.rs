use crate::ffmpeg;
use crate::metrics::SEGMENT_DRIFT;
use crate::segments;
use anyhow::{Context, Result};
use bytes::Bytes;
use futures::Stream;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::Poll;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, warn};

/// Транскодирует аудио AC3 → AAC в реальном времени через ffmpeg
pub struct AudioTranscoder {
    process: tokio::process::Child,
    stream: Option<ReaderStream<tokio::process::ChildStdout>>,
}

impl AudioTranscoder {
    /// Создает новый транскодер для файла
    pub async fn new(
        file_path: &Path,
        start_time: u64,
        duration: Option<u64>,
        audio_index: u32,
    ) -> Result<Self> {
        if !ffmpeg::is_ffmpeg_available() {
            anyhow::bail!(
                "ffmpeg не найден. Установите ffmpeg для транскодирования аудио: sudo apt-get install ffmpeg"
            );
        }

        let mut cmd = ffmpeg::command(
            ffmpeg::AudioTranscodeStream {
                input: file_path,
                start_time,
                audio_index,
                duration,
            }
            .to_args(),
        );

        // Перенаправляем stderr в piped для чтения ошибок
        cmd.stderr(Stdio::piped());
        cmd.stdout(Stdio::piped());

        let mut child = cmd.spawn().context("Не удалось запустить ffmpeg")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Не удалось получить stdout процесса"))?;

        // Запускаем задачу для чтения stderr и логирования ошибок
        let stderr = child.stderr.take();
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut buffer = vec![0u8; 1024];
                loop {
                    match reader.read(&mut buffer).await {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            let msg = String::from_utf8_lossy(&buffer[..n]);
                            // Broken pipe - это нормально, когда клиент закрывает соединение
                            if !msg.contains("Broken pipe") && !msg.contains("Error closing file") {
                                error!("ffmpeg stderr: {}", msg);
                            } else {
                                debug!("ffmpeg: клиент закрыл соединение (Broken pipe)");
                            }
                        }
                        Err(e) => {
                            error!("Ошибка чтения stderr ffmpeg: {}", e);
                            break;
                        }
                    }
                }
            });
        }

        info!("ffmpeg запущен для транскодирования: {:?}", file_path);

        let stream = ReaderStream::new(stdout);

        Ok(Self {
            process: child,
            stream: Some(stream),
        })
    }
}

impl Stream for AudioTranscoder {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // Используем ReaderStream для чтения данных
        if let Some(ref mut stream) = self.stream {
            match Pin::new(stream).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    debug!("Прочитано {} байт из ffmpeg", bytes.len());
                    Poll::Ready(Some(Ok(bytes)))
                }
                Poll::Ready(Some(Err(e))) => {
                    error!("Ошибка чтения из ffmpeg: {}", e);
                    Poll::Ready(Some(Err(e)))
                }
                Poll::Ready(None) => {
                    // EOF - проверяем статус процесса
                    match self.process.try_wait() {
                        Ok(Some(status)) => {
                            if status.success() {
                                info!("ffmpeg завершился успешно");
                                Poll::Ready(None)
                            } else {
                                error!("ffmpeg завершился с ошибкой: {:?}", status.code());
                                Poll::Ready(Some(Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("ffmpeg завершился с ошибкой: {:?}", status.code()),
                                ))))
                            }
                        }
                        Ok(None) => {
                            debug!("ffmpeg еще работает, но данных нет");
                            Poll::Ready(None) // Процесс еще работает
                        }
                        Err(e) => {
                            error!("Ошибка проверки статуса ffmpeg: {}", e);
                            Poll::Ready(Some(Err(e)))
                        }
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(None)
        }
    }
}

impl Drop for AudioTranscoder {
    fn drop(&mut self) {
        // `kill()` у tokio — future, в Drop её некому опросить; start_kill шлёт сигнал сразу
        if let Ok(None) = self.process.try_wait() {
            let _ = self.process.start_kill();
        }
    }
}

/// Сколько сегментов вперёд от текущей границы рана считается «скоро дойдёт» —
/// в этих пределах не имеет смысла стартовать ещё один ffmpeg.
const LOOKAHEAD_SEGMENTS: usize = 6;
/// Не более стольких параллельных ffmpeg-ранов на файл — иначе перемотки туда-сюда
/// плодят процессы без ограничения.
const MAX_RUNS_PER_FILE: usize = 2;
/// Лимит ранов на весь сервер по умолчанию (FFMPEG_MAX_RUNS).
pub const DEFAULT_MAX_RUNS: usize = 4;
/// Сколько ждать появления сегмента, прежде чем считать генерацию зависшей.
const SEGMENT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Один запущенный ffmpeg, наполняющий кэш сегментов начиная с `start_index`.
struct Run {
    start_index: usize,
    // Хранится, чтобы cleanup/вытеснение могли оборвать процесс через KillOnDropChild.
    task: tokio::task::JoinHandle<()>,
    failed: Arc<StdMutex<Option<String>>>,
}

/// Кэш сегментов одного файла в одном профиле потока.
struct FileCache {
    dir: PathBuf,
    profile: segments::StreamProfile,
    runs: Vec<Run>,
    /// Задача, готовящая `init.mp4`. Живёт отдельно от `runs`: init-ран пишет в свой
    /// каталог и сегментов в кэше не оставляет, поэтому `run_covering` не должен его
    /// находить — иначе запрос сегмента ждал бы ран, который ему ничего не напишет.
    init: Option<Arc<StdMutex<Option<String>>>>,
    last_access: Instant,
}

impl FileCache {
    /// Наибольший `i`, для которого на диске есть все сегменты рана от `start_index` до `i`.
    fn frontier(&self, start_index: usize) -> usize {
        let mut i = start_index;
        while self
            .dir
            .join(segments::segment_name(i, self.profile))
            .exists()
        {
            i += 1;
        }
        i.saturating_sub(1).max(start_index)
    }

    /// Индекс рана, который скоро дойдёт до `index` (уже идёт или обгонит его в пределах lookahead).
    fn run_covering(&self, index: usize) -> Option<usize> {
        self.runs.iter().position(|r| {
            r.start_index <= index && index <= self.frontier(r.start_index) + LOOKAHEAD_SEGMENTS
        })
    }
}

/// Ключ кэша: id файла, профиль и отпечаток самого файла (путь + mtime + размер). Отпечаток
/// обязателен — иначе подменённый на диске файл (или переиспользованный после пересканирования
/// id) молча отдавался бы старыми сегментами из уцелевшей temp-директории.
fn cache_key(file_id: u64, profile: segments::StreamProfile, file_path: &Path) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file_path.hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(file_path) {
        meta.len().hash(&mut hasher);
        if let Ok(mtime) = meta.modified() {
            mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .hash(&mut hasher);
        }
    }
    format!("{file_id}-{}-{:016x}", profile.cache_key(), hasher.finish())
}

/// Управляет кэшами сегментов по файлу и профилю: генерирует init/сегменты лениво,
/// переиспользуя уже запущенные ffmpeg-раны, и чистит простаивающие кэши по TTL.
///
/// Внешняя `HashMap` под мьютексом — только для поиска/создания записи; вся работа с
/// конкретным файлом идёт под его собственным мьютексом, чтобы разные фильмы не блокировали
/// друг друга.
#[derive(Clone)]
pub struct HlsManager {
    caches: Arc<AsyncMutex<HashMap<String, Arc<AsyncMutex<FileCache>>>>>,
    /// Корень файлового кэша сегментов. Вынесен в поле ради тестов: ключ кэша — отпечаток
    /// файла, поэтому при общем `/tmp` прогон подхватывал бы сегменты, нарезанные предыдущей
    /// версией кода, и проверял бы не то, что собрал сейчас.
    root: PathBuf,
    /// Сколько ранов может жить одновременно на всех файлах; сверх лимита новый ран
    /// вытесняет раны файла, к которому дольше всех не обращались.
    max_runs: usize,
}

impl HlsManager {
    pub fn new(max_runs: usize) -> Self {
        Self {
            max_runs: max_runs.max(1),
            ..Self::with_root(std::env::temp_dir())
        }
    }

    /// Корень файлового кэша — нужен тестам, чтобы заглянуть, что ран реально записал.
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self {
            caches: Arc::new(AsyncMutex::new(HashMap::new())),
            root,
            max_runs: DEFAULT_MAX_RUNS,
        }
    }

    async fn cache_for(
        &self,
        file_id: u64,
        file_path: &Path,
        profile: segments::StreamProfile,
    ) -> Arc<AsyncMutex<FileCache>> {
        let key = cache_key(file_id, profile, file_path);
        let root = self.root.clone();
        let mut caches = self.caches.lock().await;
        caches
            .entry(key.clone())
            .or_insert_with(|| {
                let dir = root.join(format!("aurora-hls-{key}"));
                let _ = std::fs::create_dir_all(&dir);
                Arc::new(AsyncMutex::new(FileCache {
                    dir,
                    profile,
                    runs: Vec::new(),
                    init: None,
                    last_access: Instant::now(),
                }))
            })
            .clone()
    }

    /// Гарантирует наличие `init.mp4` и возвращает его содержимое.
    pub async fn init_segment(
        &self,
        file_id: u64,
        file_path: &Path,
        profile: segments::StreamProfile,
        map: &segments::SegmentMap,
    ) -> Result<Vec<u8>> {
        let cache = self.cache_for(file_id, file_path, profile).await;
        self.ensure_and_wait(&cache, file_path, map, profile, 0, true)
            .await
    }

    /// Гарантирует наличие сегмента `index` и возвращает его содержимое.
    pub async fn segment(
        &self,
        file_id: u64,
        file_path: &Path,
        profile: segments::StreamProfile,
        map: &segments::SegmentMap,
        index: usize,
    ) -> Result<Vec<u8>> {
        let cache = self.cache_for(file_id, file_path, profile).await;
        self.ensure_and_wait(&cache, file_path, map, profile, index, false)
            .await
    }

    async fn ensure_and_wait(
        &self,
        cache: &Arc<AsyncMutex<FileCache>>,
        file_path: &Path,
        map: &segments::SegmentMap,
        profile: segments::StreamProfile,
        index: usize,
        is_init: bool,
    ) -> Result<Vec<u8>> {
        let dir = cache.lock().await.dir.clone();
        let target_path = if is_init {
            dir.join("init.mp4")
        } else {
            dir.join(segments::segment_name(index, profile))
        };

        // Метка запроса для трассировки: по ней в логе сходятся решение (hit/ран) и итог.
        // Кэш назван по файлу и профилю — этого хватает, чтобы не путать параллельные просмотры.
        let cache_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let what = if is_init {
            format!("{cache_name} init")
        } else {
            format!("{cache_name} seg {index}")
        };
        let started = Instant::now();

        if let Ok(data) = tokio::fs::read(&target_path).await {
            cache.lock().await.last_access = Instant::now();
            info!("HLS {what}: hit ({} байт)", data.len());
            return Ok(data);
        }

        let mut started_run = false;
        let failed = {
            let mut guard = cache.lock().await;
            guard.last_access = Instant::now();

            guard.runs.retain(|r| !r.task.is_finished());

            // init готовится отдельной короткой задачей, а не полноразмерным раном:
            // раньше запрос init при пустом кэше запускал ffmpeg с нулевого сегмента,
            // и при старте с середины файла машина параллельно перекодировала весь фильм
            // с начала — работу, которую никто не смотрит (замер: ран с 0 дошёл до
            // сегмента 205, пока зритель смотрел со 104-го).
            if is_init {
                if let Some(failed) = guard.init.clone() {
                    info!("HLS {what}: miss, ждём init-задачу");
                    failed
                } else {
                    info!("HLS {what}: miss, новая init-задача");
                    let failed = Arc::new(StdMutex::new(None));
                    let failed_for_task = failed.clone();
                    let dir = guard.dir.clone();
                    let file_path = file_path.to_path_buf();

                    let task = tokio::spawn(async move {
                        if let Err(e) = spawn_init_run(&file_path, &dir, profile).await {
                            error!("Ошибка init-рана HLS: {e}");
                            *failed_for_task.lock().unwrap() = Some(e.to_string());
                        }
                    });
                    // Хендл не храним: задача короткая и самозавершающаяся, а её падение
                    // видно через `failed`.
                    drop(task);
                    guard.init = Some(failed.clone());
                    failed
                }
            } else if let Some(run_pos) = guard.run_covering(index) {
                let r = &guard.runs[run_pos];
                let start = r.start_index;
                info!(
                    "HLS {what}: miss, ждём ран start={start} (frontier={})",
                    guard.frontier(start)
                );
                guard.runs[run_pos].failed.clone()
            } else {
                let existing: Vec<usize> = guard.runs.iter().map(|r| r.start_index).collect();
                if guard.runs.len() >= MAX_RUNS_PER_FILE {
                    let oldest = guard.runs.remove(0);
                    warn!(
                        "HLS {what}: лимит ранов ({MAX_RUNS_PER_FILE}) — глушу ран start={} (живые были {existing:?})",
                        oldest.start_index
                    );
                    oldest.task.abort();
                }

                let start_index = index;
                info!("HLS {what}: miss, новый ран start={start_index} (живые {existing:?})");
                let failed = Arc::new(StdMutex::new(None));
                let failed_for_task = failed.clone();
                let dir = guard.dir.clone();
                let file_path = file_path.to_path_buf();
                let map = map.clone();

                let task = tokio::spawn(async move {
                    let result = spawn_run(&file_path, &dir, &map, start_index, profile).await;
                    match result {
                        Ok(()) => {
                            debug!("ffmpeg-ран (start={start_index}) для {dir:?} завершён успешно");
                        }
                        Err(e) => {
                            error!("Ошибка ffmpeg-рана HLS: {e}");
                            *failed_for_task.lock().unwrap() = Some(e.to_string());
                        }
                    }
                });

                guard.runs.push(Run {
                    start_index,
                    task,
                    failed: failed.clone(),
                });
                started_run = true;
                failed
            }
        };

        // Вне блокировки своего кэша: порядок «caches → cache» тот же, что в cleanup_idle
        if started_run {
            self.enforce_run_limit(cache).await;
        }

        let deadline = Instant::now() + SEGMENT_WAIT_TIMEOUT;
        loop {
            // Проверка ошибки идёт первой: у оборвавшегося рана на диске мог остаться
            // недописанный хвостовой сегмент, который выглядит готовым.
            if let Some(msg) = failed.lock().unwrap().clone() {
                warn!(
                    "HLS {what}: ран упал через {:.1}с — {msg}",
                    started.elapsed().as_secs_f64()
                );
                anyhow::bail!("{msg}");
            }
            if let Ok(data) = tokio::fs::read(&target_path).await {
                info!(
                    "HLS {what}: готов за {:.1}с ({} байт)",
                    started.elapsed().as_secs_f64(),
                    data.len()
                );
                return Ok(data);
            }
            if Instant::now() >= deadline {
                warn!("HLS {what}: таймаут ожидания");
                anyhow::bail!(
                    "сегмент не сгенерирован за {}с",
                    SEGMENT_WAIT_TIMEOUT.as_secs()
                );
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Глушит раны давно не запрашивавшихся файлов, пока живых ранов не станет не больше
    /// `max_runs`. Раны брошенного просмотра идут до конца файла, пока кэш не снесёт
    /// cleanup_idle, — без вытеснения они занимали бы CPU вместо тех, кого смотрят сейчас.
    /// Ожидающим вытесненного рана сразу отдаётся ошибка, а не таймаут.
    async fn enforce_run_limit(&self, current: &Arc<AsyncMutex<FileCache>>) {
        let caches = self.caches.lock().await;
        let mut candidates = Vec::new();
        let mut live = 0;
        for cache in caches.values() {
            let mut guard = cache.lock().await;
            guard.runs.retain(|r| !r.task.is_finished());
            live += guard.runs.len();
            if !guard.runs.is_empty() && !Arc::ptr_eq(cache, current) {
                candidates.push((guard.last_access, cache.clone()));
            }
        }
        candidates.sort_by_key(|(last_access, _)| *last_access);

        for (_, cache) in candidates {
            if live <= self.max_runs {
                break;
            }
            let mut guard = cache.lock().await;
            warn!(
                "HLS: лимит ранов на сервер ({}) — глушу {} ран(ов) в {:?}",
                self.max_runs,
                guard.runs.len(),
                guard.dir
            );
            live -= guard.runs.len();
            for run in guard.runs.drain(..) {
                run.task.abort();
                *run.failed.lock().unwrap() = Some("ран вытеснен лимитом ffmpeg".to_string());
            }
        }
    }

    /// Удаляет кэши, простаивающие дольше `ttl` (глуша живые раны), вместе с их
    /// temp-директориями.
    pub async fn cleanup_idle(&self, ttl: Duration) {
        let mut caches = self.caches.lock().await;
        let mut stale = Vec::new();
        for (key, cache) in caches.iter() {
            let guard = cache.lock().await;
            if guard.last_access.elapsed() > ttl {
                stale.push(key.clone());
            }
        }

        for key in stale {
            if let Some(cache) = caches.remove(&key) {
                let guard = cache.lock().await;
                info!("Удаляю простаивающий HLS-кэш {:?}: {:?}", key, guard.dir);
                for run in &guard.runs {
                    run.task.abort();
                }
                let _ = std::fs::remove_dir_all(&guard.dir);
            }
        }
    }
}

impl Default for HlsManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RUNS)
    }
}

/// Сообщения декодеров/демуксеров о битых пакетах в исходнике. ffmpeg их отбрасывает и
/// продолжает, выход остаётся валидным — фатальность рана определяется только exit-кодом.
/// Частый источник: DVD-рипы в AVI с мусором перед первым валидным MP3-фреймом.
const RECOVERABLE_PATTERNS: &[&str] = &[
    "Header missing",
    "Error submitting packet to decoder",
    "Invalid data found when processing input",
    "corrupt",
    "concealing",
    "non-existing PPS",
    "no frame!",
    "mmco:",
    "Past duration",
    "Non-monotonous DTS",
    "Non-monotonic DTS",
    "Application provided invalid, non monotonically increasing dts",
    "skipped MB",
    "error while decoding MB",
];

fn is_recoverable(line: &str) -> bool {
    RECOVERABLE_PATTERNS.iter().any(|p| line.contains(p))
}

/// Запускает один ffmpeg-ран, наполняющий `dir` сегментами начиная с `start_index`, по карте
/// `map`.
///
/// Способ нарезки выбирается не флагом, а вариантом карты: сетка режется hls-муксером
/// (keyframes заказываем сами), keyframes-карта — посегментно. Несовпадение карты и профиля
/// сюда дойти не должно — `segment_map_for` его отсекает.
async fn spawn_run(
    file_path: &Path,
    dir: &Path,
    map: &segments::SegmentMap,
    start_index: usize,
    profile: segments::StreamProfile,
) -> Result<()> {
    if !profile.uses_fmp4() {
        let Some(boundaries) = map.as_keyframes() else {
            anyhow::bail!("для профиля {} нужна карта по keyframes", profile.key());
        };
        return run_copy_segments(file_path, dir, boundaries, start_index, profile).await;
    }
    let start_time = map.start_of(start_index).unwrap_or(0.0);

    let init_name = format!("init.{start_index}.tmp.mp4");
    let mut cmd = ffmpeg::command(
        ffmpeg::HlsEncodeRun {
            input: file_path,
            dir,
            start_time,
            start_index,
            transcode_audio: profile.transcode_audio,
            audio_index: profile.audio_index,
            deinterlace: profile.deinterlace,
            max_seconds: None,
            segment_seconds: segments::GRID_SEGMENT_SECONDS,
            init_name: &init_name,
        }
        .to_args(),
    );

    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let child = cmd.spawn().context("Не удалось запустить ffmpeg для HLS")?;
    // Убивает ffmpeg, если эта future будет отменена (task.abort()) — иначе осиротевший
    // процесс продолжает писать в dir.
    let mut child = KillOnDropChild(child);

    log_stderr(&mut child);

    info!(
        "ffmpeg HLS-ран запущен для {:?} (dir={:?}, start_index={})",
        file_path, dir, start_index
    );

    let dir_for_init = dir.to_path_buf();
    tokio::spawn(async move {
        publish_init(&dir_for_init, &init_name, start_index, profile).await;
    });

    let status = child
        .0
        .wait()
        .await
        .context("Ошибка ожидания завершения ffmpeg (hls)")?;
    if !status.success() {
        anyhow::bail!("ffmpeg завершился с кодом {:?}", status.code());
    }
    Ok(())
}

/// Готовит только `init.mp4` — заголовок fMP4 с параметрами кодека.
///
/// Пишет в собственный подкаталог и копирует наружу единственный файл. Каталог отдельный,
/// потому что ран, ограниченный `-t`, оставляет после себя обрезанный `segment00000.m4s`:
/// попади он в кэш, зритель получил бы вместо шестисекундного сегмента односекундный.
///
/// init зависит только от настроек кодировщика, а не от позиции, поэтому его можно готовить
/// раном с начала файла независимо от того, откуда начнётся просмотр.
async fn spawn_init_run(
    file_path: &Path,
    dir: &Path,
    profile: segments::StreamProfile,
) -> Result<()> {
    let final_path = dir.join("init.mp4");
    if final_path.exists() {
        return Ok(());
    }

    let scratch = dir.join("init-scratch");
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    tokio::fs::create_dir_all(&scratch)
        .await
        .context("Не удалось создать каталог для init-рана")?;

    let init_name = "init.mp4";
    let mut cmd = ffmpeg::command(
        ffmpeg::HlsEncodeRun {
            input: file_path,
            dir: &scratch,
            start_time: 0.0,
            start_index: 0,
            transcode_audio: profile.transcode_audio,
            audio_index: profile.audio_index,
            deinterlace: profile.deinterlace,
            // Один сегмент: меньше — риск, что муксер не успеет дописать заголовок.
            max_seconds: Some(segments::GRID_SEGMENT_SECONDS),
            segment_seconds: segments::GRID_SEGMENT_SECONDS,
            init_name,
        }
        .to_args(),
    );
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let child = cmd
        .spawn()
        .context("Не удалось запустить ffmpeg для init")?;
    let mut child = KillOnDropChild(child);
    log_stderr(&mut child);

    info!("ffmpeg init-ран запущен для {file_path:?} (dir={scratch:?})");

    let status = child
        .0
        .wait()
        .await
        .context("Ошибка ожидания завершения ffmpeg (init)")?;
    if !status.success() {
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        anyhow::bail!("ffmpeg (init) завершился с кодом {:?}", status.code());
    }

    let produced = scratch.join(init_name);
    if !produced.exists() {
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        anyhow::bail!("init-ран не создал {init_name}");
    }
    // Публикуем через rename в пределах одного каталога — читатель не увидит недописанный файл.
    let tmp = dir.join("init.tmp.mp4");
    tokio::fs::copy(&produced, &tmp)
        .await
        .context("Не удалось скопировать init.mp4")?;
    let _ = tokio::fs::rename(&tmp, &final_path).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;

    Ok(())
}

/// Наполняет `dir` TS-сегментами начиная с `start_index`, по одному ffmpeg на сегмент.
///
/// Принимает именно `Boundaries` из `SegmentMap::Keyframes`: равномерная сетка здесь означала бы
/// резы не по keyframe и тихий рассинхрон, поэтому её сюда не передать — это проверяет тип, а не
/// рантайм. Рецепт вызова — в `ffmpeg::CopySegment`.
async fn run_copy_segments(
    file_path: &Path,
    dir: &Path,
    map: &segments::Boundaries,
    start_index: usize,
    profile: segments::StreamProfile,
) -> Result<()> {
    info!(
        "ffmpeg HLS-ран (copy) запущен для {:?} (dir={:?}, start_index={})",
        file_path, dir, start_index
    );

    for index in start_index..map.count() {
        let (Some(from), Some(len)) = (map.start_of(index), map.duration_of(index)) else {
            break;
        };
        let target = dir.join(segments::segment_name(index, profile));
        if target.exists() {
            continue;
        }
        // Имя временного файла уникально для рана: параллельный ран, догнавший этот же индекс,
        // иначе упёрся бы в «File ... already exists. Exiting.».
        let tmp = dir.join(format!("segment{index:05}.{start_index}.tmp.ts"));

        let mut cmd = ffmpeg::command(
            ffmpeg::CopySegment {
                input: file_path,
                from,
                duration: len,
                transcode_audio: profile.transcode_audio,
                audio_index: profile.audio_index,
                output: tmp.clone(),
            }
            .to_args(),
        );
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let child = cmd.spawn().context("Не удалось запустить ffmpeg для HLS")?;
        let mut child = KillOnDropChild(child);
        log_stderr(&mut child);
        let status = child
            .0
            .wait()
            .await
            .context("Ошибка ожидания завершения ffmpeg (hls)")?;
        if !status.success() {
            let _ = tokio::fs::remove_file(&tmp).await;
            anyhow::bail!(
                "ffmpeg завершился с кодом {:?} на сегменте {index}",
                status.code()
            );
        }
        // Пока шёл ffmpeg, сегмент мог опубликовать другой ран — свой результат выбрасываем.
        if target.exists() {
            let _ = tokio::fs::remove_file(&tmp).await;
            continue;
        }
        // Готовый сегмент появляется под финальным именем атомарно — читатель не увидит
        // недописанный файл.
        tokio::fs::rename(&tmp, &target)
            .await
            .with_context(|| format!("Не удалось опубликовать сегмент {index}"))?;

        verify_segment_duration(target, index, len);
    }

    Ok(())
}

/// Проверяет инвариант «сегмент длится ровно столько, сколько обещает карта».
///
/// Именно его нарушение и было тем самым тихим багом: ffmpeg отрабатывал успешно, HTTP отдавал
/// 200, в логах ничего — а плеер подвисал, потому что EXTINF в плейлисте расходился с реальным
/// содержимым. Проверка идёт в фоне (ffprobe готового TS дешевле нарезки на порядок) и копит
/// счётчик в `/api/metrics`: ненулевой `mismatched` означает баг, даже если никто не жалуется.
fn verify_segment_duration(path: PathBuf, index: usize, expected: f64) {
    tokio::spawn(async move {
        let actual = match tokio::task::spawn_blocking(move || {
            ffmpeg::probe_video_span_seconds(&path)
        })
        .await
        {
            Ok(Ok(actual)) => actual,
            // Не смогли измерить — это не признак рассинхрона, счётчик не трогаем.
            _ => return,
        };
        if let Some(delta) = SEGMENT_DRIFT.record(expected, actual) {
            warn!(
                "Сегмент {index} разошёлся с картой: ожидалось {expected:.3}с, получено {actual:.3}с (Δ{delta:.3}с)"
            );
        }
    });
}

/// Сливает stderr ffmpeg в лог: восстановимые ошибки декодера — в debug со сводкой в конце.
fn log_stderr(child: &mut KillOnDropChild) {
    let Some(stderr) = child.0.stderr.take() else {
        return;
    };
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        let mut recoverable = 0usize;
        loop {
            match lines.next_line().await {
                Ok(None) => break,
                Ok(Some(line)) if line.trim().is_empty() => {}
                Ok(Some(line)) => {
                    if is_recoverable(&line) {
                        recoverable += 1;
                        debug!("ffmpeg (hls) stderr: {line}");
                    } else {
                        error!("ffmpeg (hls) stderr: {line}");
                    }
                }
                Err(e) => {
                    error!("Ошибка чтения stderr ffmpeg (hls): {e}");
                    break;
                }
            }
        }
        if recoverable > 0 {
            warn!(
                "ffmpeg (hls): {recoverable} восстановимых ошибок декодера (битые пакеты в исходнике)"
            );
        }
    });
}

/// Ждёт, пока ран допишет init и перейдёт к сегментам, и публикует init как общий `init.mp4` —
/// только если тот ещё не опубликован другим раном (содержимое одинаково между ранами, важно
/// лишь, чтобы клиент никогда не прочитал полузаписанный или подменённый файл).
///
/// Дожидаемся не просто существования `tmp_name` — `-hls_flags temp_file` защищает от чтения
/// недописанного файла только сегменты, init ffmpeg пишет напрямую под финальным именем. Первый
/// появившийся сегмент рана — надёжный сигнал, что init уже полностью сброшен на диск.
async fn publish_init(
    dir: &Path,
    tmp_name: &str,
    start_index: usize,
    profile: segments::StreamProfile,
) {
    let tmp_path = dir.join(tmp_name);
    let first_segment = dir.join(segments::segment_name(start_index, profile));
    let final_path = dir.join("init.mp4");

    let deadline = Instant::now() + SEGMENT_WAIT_TIMEOUT;
    while !first_segment.exists() {
        if final_path.exists() {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    if final_path.exists() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return;
    }

    if tokio::fs::rename(&tmp_path, &final_path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
}

/// Оборачивает дочерний процесс и убивает его при уничтожении, если он ещё жив —
/// нужно, чтобы отмена future (запрос оборван клиентом) не оставляла ffmpeg висеть.
struct KillOnDropChild(tokio::process::Child);

impl Drop for KillOnDropChild {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: segments::StreamProfile = segments::StreamProfile {
        transcode_video: false,
        transcode_audio: false,
        deinterlace: false,
        audio_index: 0,
    };

    async fn add_run(cache: &Arc<AsyncMutex<FileCache>>) -> Arc<StdMutex<Option<String>>> {
        let failed = Arc::new(StdMutex::new(None));
        let mut guard = cache.lock().await;
        guard.last_access = Instant::now();
        guard.runs.push(Run {
            start_index: 0,
            task: tokio::spawn(std::future::pending()),
            failed: failed.clone(),
        });
        failed
    }

    #[tokio::test]
    async fn run_limit_evicts_least_recently_used_file() {
        let root = std::env::temp_dir().join(format!("aurora-limit-{}", std::process::id()));
        let hls = HlsManager {
            max_runs: 2,
            ..HlsManager::with_root(root.clone())
        };
        let a = hls.cache_for(1, Path::new("/a.mkv"), PROFILE).await;
        let b = hls.cache_for(2, Path::new("/b.mkv"), PROFILE).await;
        let c = hls.cache_for(3, Path::new("/c.mkv"), PROFILE).await;

        let failed_a = add_run(&a).await;
        let failed_b = add_run(&b).await;
        add_run(&c).await;
        hls.enforce_run_limit(&c).await;

        assert!(a.lock().await.runs.is_empty(), "самый старый файл вытеснен");
        assert!(failed_a.lock().unwrap().is_some(), "ожидающие получают ошибку");
        assert_eq!(b.lock().await.runs.len(), 1);
        assert!(failed_b.lock().unwrap().is_none());
        assert_eq!(c.lock().await.runs.len(), 1, "свой ран не трогаем");

        let _ = std::fs::remove_dir_all(root);
    }
}
