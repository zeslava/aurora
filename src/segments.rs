//! Карта сегментов HLS: детерминированное разбиение файла на сегменты и рендер
//! VOD-плейлиста.
//!
//! Ключевая идея: границы сегментов — чистая функция от файла и профиля потока, а не от
//! того, кто и с какой позиции смотрит. Поэтому плейлист можно отдать целиком (со всеми
//! `#EXTINF` и `#EXT-X-ENDLIST`) сразу, ещё до того как хоть один сегмент закодирован,
//! а сами сегменты генерировать лениво по номеру. Для плеера это обычный VOD: он знает
//! длительность, видит весь диапазон seekable и не включает live-логику с репозиционированием
//! playhead (из-за неё поток «зацикливался» на отрезке).

use crate::ffmpeg;
use anyhow::{Context, Result};
use std::path::Path;

/// Целевая длина сегмента при перекодировании видео: границы задаём сами через
/// `-force_key_frames`, поэтому сетка ровная.
pub const GRID_SEGMENT_SECONDS: f64 = 6.0;

/// Минимальная длина сегмента при `-c:v copy`: резать можно только по keyframe, поэтому
/// берём первый keyframe не ближе этого порога. Реальная длина — от порога до порога плюс GOP.
const MIN_COPY_SEGMENT_SECONDS: f64 = 4.0;

/// Хвост короче этого сливаем с предыдущим сегментом: односекундные огрызки в конце
/// плейлиста некоторые плееры отрабатывают криво.
const MIN_TAIL_SECONDS: f64 = 1.0;

/// Профиль потока — что именно ffmpeg делает с дорожками. Выводится только из кодеков
/// файла, поэтому одинаков для всех клиентов: телевизор и планшет делят один кэш сегментов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamProfile {
    pub transcode_video: bool,
    pub transcode_audio: bool,
    /// Источник чересстрочный — видео надо не просто перекодировать, а сначала собрать
    /// кадры из полей. Всегда вместе с `transcode_video`: класть поля в fMP4 через
    /// `-c:v copy` нельзя, MSE их не декодирует.
    pub deinterlace: bool,
    /// Номер аудиодорожки среди аудиопотоков файла (`-map 0:a:N`). У релизов с несколькими
    /// переводами их бывают десятки; выбор приходит от клиента параметром `?audio=`.
    pub audio_index: u32,
}

impl StreamProfile {
    /// Стабильный строковый ключ профиля — первичный ключ карты сегментов в БД.
    /// Ни деинтерлейс, ни номер аудиодорожки в него не входят: границы сегментов задаёт
    /// видео, а от этих полей оно не зависит — карта в profile "v"/"av" одинакова
    /// с ними и без них. Для каталога сегментов нужен [`Self::cache_key`].
    pub fn key(&self) -> &'static str {
        match (self.transcode_video, self.transcode_audio) {
            (false, false) => "copy",
            (false, true) => "a",
            (true, false) => "v",
            (true, true) => "av",
        }
    }

    /// Ключ каталога сгенерированных сегментов. В отличие от [`Self::key`] включает
    /// аудиодорожку: сами сегменты у разных дорожек разные, смешивать их в одном каталоге
    /// нельзя.
    pub fn cache_key(&self) -> String {
        format!("{}-a{}", self.key(), self.audio_index)
    }

    /// Контейнер сегментов. fMP4 — только когда видео перекодируется: там границы задаём сами
    /// (`-force_key_frames`), и hls-муксер режет ровно по ним. При `-c:v copy` резать можно
    /// только по keyframe источника, и единственный муксер, который принимает готовый список
    /// точек реза, — `segment`; с ним берём MPEG-TS: он не требует init-сегмента и одинаков
    /// для всех ранов. H.264 (единственный кодек, доживающий до copy) в TS кладётся штатно.
    pub fn uses_fmp4(&self) -> bool {
        self.transcode_video
    }
}

/// Границы сегментов в секундах от начала файла: `boundaries[i]` — начало сегмента `i`,
/// `boundaries[i + 1]` — его конец. Длина всегда на единицу больше числа сегментов.
///
/// Сам по себе тип ничего не говорит о том, откуда границы взялись — это несёт [`SegmentMap`].
#[derive(Debug, Clone, PartialEq)]
pub struct Boundaries {
    boundaries: Vec<f64>,
}

/// Карта сегментов вместе с происхождением границ. Разделение не косметическое:
/// границы по равномерной сетке допустимы **только** когда видео перекодируется и keyframes
/// ставим мы сами. Отдать такую карту в `-c:v copy` — гарантированный рассинхрон
/// (см. `docs/decisions/0001-hls-copy-segmentation.md`), поэтому нарезчик copy-профиля
/// принимает `Keyframes` на уровне типа, а не проверяет флаг в рантайме.
#[derive(Debug, Clone, PartialEq)]
pub enum SegmentMap {
    /// Границы совпадают с keyframes исходника — годятся для `-c:v copy`.
    Keyframes(Boundaries),
    /// Равномерная сетка — годится только там, где keyframes задаём мы (`-force_key_frames`).
    Grid(Boundaries),
}

impl SegmentMap {
    pub fn boundaries(&self) -> &Boundaries {
        match self {
            Self::Keyframes(b) | Self::Grid(b) => b,
        }
    }

    /// Границы для `-c:v copy`. `None` для сетки — вызывающий обязан обработать явно.
    pub fn as_keyframes(&self) -> Option<&Boundaries> {
        match self {
            Self::Keyframes(b) => Some(b),
            Self::Grid(_) => None,
        }
    }

    /// Подходит ли карта профилю. Профиль однозначно определяет допустимое происхождение:
    /// перекодируем видео → сетка, копируем → keyframes.
    pub fn matches(&self, profile: StreamProfile) -> bool {
        matches!(
            (self, profile.transcode_video),
            (Self::Grid(_), true) | (Self::Keyframes(_), false)
        )
    }

    pub fn count(&self) -> usize {
        self.boundaries().count()
    }

    pub fn start_of(&self, index: usize) -> Option<f64> {
        self.boundaries().start_of(index)
    }

    pub fn to_playlist(&self, profile: StreamProfile) -> String {
        self.boundaries().to_playlist(profile)
    }

    /// Сериализация несёт происхождение границ. Старый формат (голый массив) не читается
    /// намеренно: по нему нельзя восстановить, keyframes это или сетка, — такие записи
    /// считаются промахом кэша и карта строится заново.
    pub fn to_json(&self) -> String {
        let (source, b) = match self {
            Self::Keyframes(b) => ("keyframes", b),
            Self::Grid(b) => ("grid", b),
        };
        serde_json::json!({ "source": source, "boundaries": b.boundaries }).to_string()
    }

    pub fn from_json(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let boundaries: Vec<f64> = serde_json::from_value(value.get("boundaries")?.clone()).ok()?;
        if boundaries.len() < 2 {
            return None;
        }
        let b = Boundaries { boundaries };
        match value.get("source")?.as_str()? {
            "keyframes" => Some(Self::Keyframes(b)),
            "grid" => Some(Self::Grid(b)),
            _ => None,
        }
    }
}

impl Boundaries {
    /// Создаёт карту из списка границ, приводя его к корректному виду
    /// (сортировка, удаление дублей, обязательные 0 и `duration` по краям).
    fn from_boundaries(mut boundaries: Vec<f64>, duration: f64) -> Self {
        boundaries.retain(|t| t.is_finite() && *t > 0.0 && *t < duration);
        boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        boundaries.dedup();

        let mut result = Vec::with_capacity(boundaries.len() + 2);
        result.push(0.0);
        result.extend(boundaries);
        result.push(duration);

        // Слишком короткий хвост сливаем с предыдущим сегментом
        if result.len() > 2 {
            let last = result[result.len() - 1];
            let prev = result[result.len() - 2];
            if last - prev < MIN_TAIL_SECONDS {
                result.remove(result.len() - 2);
            }
        }

        Self { boundaries: result }
    }

    pub fn count(&self) -> usize {
        self.boundaries.len().saturating_sub(1)
    }

    /// Начало сегмента `index` в секундах.
    pub fn start_of(&self, index: usize) -> Option<f64> {
        if index >= self.count() {
            return None;
        }
        Some(self.boundaries[index])
    }

    /// Длительность сегмента `index` в секундах.
    pub fn duration_of(&self, index: usize) -> Option<f64> {
        if index >= self.count() {
            return None;
        }
        Some(self.boundaries[index + 1] - self.boundaries[index])
    }

    /// Рендерит VOD-плейлист. `#EXT-X-ENDLIST` здесь принципиален: без него hls.js считает
    /// поток live и начинает сам двигать позицию воспроизведения.
    pub fn to_playlist(&self, profile: StreamProfile) -> String {
        let target = self
            .boundaries
            .windows(2)
            .map(|w| w[1] - w[0])
            .fold(0.0_f64, f64::max)
            .ceil()
            .max(1.0) as u64;

        let mut out = String::with_capacity(self.count() * 48 + 200);
        out.push_str("#EXTM3U\n");
        // Версия 7 нужна только ради `#EXT-X-MAP` с fMP4; TS-сегментам хватает 3.
        out.push_str(if profile.uses_fmp4() {
            "#EXT-X-VERSION:7\n"
        } else {
            "#EXT-X-VERSION:3\n"
        });
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target}\n"));
        out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
        out.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
        out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        if profile.uses_fmp4() {
            out.push_str(&format!(
                "#EXT-X-MAP:URI=\"init.mp4?p={}&a={}\"\n",
                profile.key(),
                profile.audio_index
            ));
        }

        for i in 0..self.count() {
            let dur = self.duration_of(i).unwrap_or(0.0);
            out.push_str(&format!("#EXTINF:{dur:.6},\n"));
            out.push_str(&format!("{}\n", segment_uri(i, profile)));
        }

        out.push_str("#EXT-X-ENDLIST\n");
        out
    }
}

/// Расширение файла сегмента в зависимости от контейнера профиля.
pub fn segment_extension(profile: StreamProfile) -> &'static str {
    if profile.uses_fmp4() { "m4s" } else { "ts" }
}

/// Имя файла сегмента по его номеру. Единственное место, где номер превращается в имя —
/// плейлист и генератор обязаны использовать одну и ту же схему.
pub fn segment_name(index: usize, profile: StreamProfile) -> String {
    format!("segment{index:05}.{}", segment_extension(profile))
}

/// URI сегмента в плейлисте: имя файла плюс профиль, при котором плейлист был отрендерен.
///
/// Профиль в запросе нужен, чтобы отличить устаревший плейлист от битого запроса. VOD-манифест
/// hls.js загружает один раз и никогда не перезапрашивает, поэтому после смены профиля файла
/// (например, когда бэкфилл `field_order` перевёл его с copy на перекодирование) живой плеер
/// продолжает просить сегменты старого контейнера. Без этого маркера сервер видел лишь
/// неразбираемое имя и отвечал 400, а плеер молча ретраил его до фатальной ошибки.
pub fn segment_uri(index: usize, profile: StreamProfile) -> String {
    format!(
        "{}?p={}&a={}",
        segment_name(index, profile),
        profile.key(),
        profile.audio_index
    )
}

/// Разбирает имя сегмента обратно в номер. Возвращает `None` для чужих имён —
/// заодно служит защитой от обхода каталога.
pub fn parse_segment_name(name: &str, profile: StreamProfile) -> Option<usize> {
    name.strip_prefix("segment")?
        .strip_suffix(segment_extension(profile))?
        .strip_suffix('.')?
        .parse()
        .ok()
}

/// Строит карту сегментов для файла. Тяжёлая (запускает ffprobe) — вызывать один раз
/// на файл+профиль, результат кэшировать.
pub async fn build_segment_map(
    file_path: &Path,
    duration: f64,
    profile: StreamProfile,
) -> Result<SegmentMap> {
    if !(duration.is_finite() && duration > 0.0) {
        anyhow::bail!("Неизвестная длительность файла — карту сегментов построить нельзя");
    }

    // Видео перекодируется — границы задаём сами (`-force_key_frames` в транскодере),
    // индекс keyframes не нужен и ffprobe можно не гонять.
    if profile.transcode_video {
        return Ok(SegmentMap::Grid(grid_map(duration)));
    }

    let path = file_path.to_path_buf();
    let keyframes = tokio::task::spawn_blocking(move || probe_keyframes(&path))
        .await
        .context("Задача чтения keyframes прервана")?;

    match keyframes {
        Ok(kf) if kf.len() > 1 => Ok(SegmentMap::Keyframes(collapse_keyframes(&kf, duration))),
        // Без индекса keyframes корректную карту для `-c:v copy` не построить: резать в
        // произвольной точке нельзя, а сетка разъедется с реальными резами муксера. Вызывающий
        // на эту ошибку переключает файл на профиль с перекодированием видео — там границы
        // задаём мы сами.
        Ok(_) => anyhow::bail!("в файле не найдено keyframes — карту для -c:v copy не построить"),
        Err(e) => Err(e),
    }
}

/// Равномерная сетка сегментов.
fn grid_map(duration: f64) -> Boundaries {
    let mut boundaries = Vec::new();
    let mut t = GRID_SEGMENT_SECONDS;
    while t < duration {
        boundaries.push(t);
        t += GRID_SEGMENT_SECONDS;
    }
    Boundaries::from_boundaries(boundaries, duration)
}

/// Схлопывает keyframes в границы сегментов: берём первый keyframe, отстоящий от прошлой
/// границы не меньше чем на `MIN_COPY_SEGMENT_SECONDS`. При типичном GOP 2–5 с даёт
/// сегменты 4–10 с; при редких keyframes сегменты выходят длиннее, но резать в другом
/// месте при `-c:v copy` нельзя.
fn collapse_keyframes(keyframes: &[f64], duration: f64) -> Boundaries {
    let mut boundaries = Vec::new();
    let mut last = 0.0_f64;

    for &kf in keyframes {
        if kf <= 0.0 || kf >= duration {
            continue;
        }
        if kf - last >= MIN_COPY_SEGMENT_SECONDS {
            boundaries.push(kf);
            last = kf;
        }
    }

    Boundaries::from_boundaries(boundaries, duration)
}

/// Читает времена keyframes видеопотока.
///
/// Берётся `dts`, а не `pts`: выходной `-ss` в ffmpeg отбрасывает пакет по dts, и граница,
/// поставленная в pts keyframe (при B-кадрах он на кадр-другой больше dts), выкинула бы сам
/// keyframe — сегмент начался бы со следующего, оставив дыру в видео.
fn probe_keyframes(file_path: &Path) -> Result<Vec<f64>> {
    Ok(parse_keyframe_csv(&ffmpeg::probe_video_packets_csv(
        file_path,
    )?))
}

/// Парсит вывод `ffprobe ... -of csv=p=0` вида `12.345000,12.303000,K__`, оставляя только
/// keyframes. Поля идут в порядке объявления в ffprobe (pts, dts, flags), а не в порядке
/// запроса. Время берём по dts; `N/A` (бывает у первого пакета) заменяем на pts.
fn parse_keyframe_csv(stdout: &str) -> Vec<f64> {
    let mut times = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.trim().split(',');
        let Some(pts) = parts.next() else { continue };
        let Some(dts) = parts.next() else { continue };
        let Some(flags) = parts.next() else { continue };
        if !flags.starts_with('K') {
            continue;
        }
        if let Ok(t) = dts.parse::<f64>().or_else(|_| pts.parse::<f64>()) {
            if t.is_finite() {
                times.push(t);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_map_covers_whole_duration() {
        let map = grid_map(20.0);
        assert_eq!(map.count(), 4);
        assert_eq!(map.start_of(0), Some(0.0));
        assert_eq!(map.duration_of(3), Some(2.0));
        assert_eq!(map.start_of(4), None);
    }

    #[test]
    fn short_tail_merges_into_previous_segment() {
        // 12.5 c при сетке 6 c дало бы хвост 0.5 c — он должен слиться с предыдущим
        let map = grid_map(12.5);
        assert_eq!(map.count(), 2);
        assert_eq!(map.duration_of(1), Some(6.5));
    }

    #[test]
    fn keyframes_collapse_into_segments_of_at_least_four_seconds() {
        // GOP 2 c на 30-секундном файле
        let keyframes: Vec<f64> = (0..15).map(|i| i as f64 * 2.0).collect();
        let map = collapse_keyframes(&keyframes, 30.0);

        assert!(map.count() > 1);
        // Последний сегмент — исключение: это хвост файла до keyframe не дотягивает,
        // резать раньше при `-c:v copy` нельзя.
        for i in 0..map.count() - 1 {
            let dur = map.duration_of(i).unwrap();
            assert!(
                dur >= MIN_COPY_SEGMENT_SECONDS,
                "сегмент {i} длиной {dur} короче минимума"
            );
        }
        // Границы обязаны совпадать с keyframes — иначе `-c:v copy` не сможет резать
        for i in 1..map.count() {
            let start = map.start_of(i).unwrap();
            assert!(
                keyframes.contains(&start),
                "граница {start} не совпадает с keyframe"
            );
        }
    }

    #[test]
    fn sparse_keyframes_produce_long_segments_instead_of_wrong_cuts() {
        // keyframe раз в 15 c — резать чаще нельзя
        let keyframes = vec![0.0, 15.0, 30.0, 45.0];
        let map = collapse_keyframes(&keyframes, 60.0);
        assert_eq!(map.count(), 4);
        assert_eq!(map.duration_of(0), Some(15.0));
    }

    const FMP4: StreamProfile = StreamProfile {
        transcode_video: true,
        transcode_audio: true,
        deinterlace: false,
        audio_index: 0,
    };
    const TS: StreamProfile = StreamProfile {
        transcode_video: false,
        transcode_audio: true,
        deinterlace: false,
        audio_index: 0,
    };

    #[test]
    fn playlist_is_vod_and_terminated() {
        let playlist = grid_map(20.0).to_playlist(FMP4);

        assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(playlist.contains("#EXT-X-ENDLIST"));
        assert!(playlist.contains("#EXT-X-MAP:URI=\"init.mp4?p="));
        assert!(playlist.contains("#EXT-X-TARGETDURATION:6"));
        assert_eq!(playlist.matches("#EXTINF:").count(), 4);
        assert!(playlist.contains("segment00000.m4s"));
        assert!(playlist.contains("segment00003.m4s"));
    }

    #[test]
    fn copy_playlist_uses_ts_without_init_map() {
        let playlist = grid_map(20.0).to_playlist(TS);

        assert!(!playlist.contains("#EXT-X-MAP"));
        assert!(playlist.contains("#EXT-X-VERSION:3"));
        assert!(playlist.contains("segment00000.ts"));
        assert!(!playlist.contains(".m4s"));
    }

    #[test]
    fn segment_name_roundtrips() {
        for profile in [FMP4, TS] {
            assert_eq!(
                parse_segment_name(&segment_name(42, profile), profile),
                Some(42)
            );
            assert_eq!(parse_segment_name("init.mp4", profile), None);
            assert_eq!(parse_segment_name("../../etc/passwd", profile), None);
            assert_eq!(parse_segment_name("segmentAB.m4s", profile), None);
        }
        // Имя из чужого контейнера не должно приниматься — иначе номер уедет в другой кэш
        assert_eq!(parse_segment_name("segment00042.ts", FMP4), None);
        assert_eq!(parse_segment_name("segment00042.m4s", TS), None);
    }

    #[test]
    fn map_survives_json_roundtrip() {
        for map in [
            SegmentMap::Grid(grid_map(37.0)),
            SegmentMap::Keyframes(collapse_keyframes(&[0.0, 15.0, 30.0], 37.0)),
        ] {
            let restored = SegmentMap::from_json(&map.to_json()).unwrap();
            assert_eq!(map, restored);
        }
    }

    #[test]
    fn legacy_json_without_source_is_treated_as_cache_miss() {
        // Голый массив границ не говорит, keyframes это или сетка: принять его — значит
        // рискнуть отдать сетку в `-c:v copy`. Такая запись должна считаться промахом кэша.
        assert!(SegmentMap::from_json("[0.0,6.0,12.0]").is_none());
    }

    #[test]
    fn map_source_must_match_profile() {
        let grid = SegmentMap::Grid(grid_map(37.0));
        let kf = SegmentMap::Keyframes(collapse_keyframes(&[0.0, 15.0, 30.0], 37.0));

        assert!(grid.matches(FMP4) && !grid.matches(TS));
        assert!(kf.matches(TS) && !kf.matches(FMP4));
        // Нарезчику copy-профиля сетка недоступна даже по ошибке — этого требует тип.
        assert!(grid.as_keyframes().is_none());
        assert!(kf.as_keyframes().is_some());
    }

    #[test]
    fn keyframe_csv_keeps_only_keyframes() {
        let stdout = "0.000000,N/A,K__\n2.042000,2.000000,__\n4.042000,4.000000,K_\nbroken\n6.042000,6.000000,K__\n";
        assert_eq!(parse_keyframe_csv(stdout), vec![0.0, 4.0, 6.0]);
    }

    /// Карта сегментов от аудиодорожки не зависит и переиспользуется между дорожками,
    /// а каталог сегментов — обязан различаться.
    #[test]
    fn audio_track_changes_cache_key_but_not_map_key() {
        let first = StreamProfile {
            audio_index: 0,
            ..FMP4
        };
        let second = StreamProfile {
            audio_index: 3,
            ..FMP4
        };
        assert_eq!(first.key(), second.key());
        assert_ne!(first.cache_key(), second.cache_key());
    }

    #[test]
    fn profile_keys_are_distinct() {
        let keys: Vec<&str> = [(false, false), (false, true), (true, false), (true, true)]
            .iter()
            .map(|&(v, a)| {
                StreamProfile {
                    transcode_video: v,
                    transcode_audio: a,
                    deinterlace: false,
                    audio_index: 0,
                }
                .key()
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len());
    }
}
