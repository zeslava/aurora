//! Единственное место, где живут вызовы ffmpeg/ffprobe и семантика их флагов.
//!
//! Зачем отдельный модуль: рецепты ffmpeg — самая дорогая в отладке часть проекта
//! (см. `docs/decisions/0001-hls-copy-segmentation.md`). Пока они были размазаны по
//! `transcoder.rs`, `metadata.rs` и `segments.rs` в виде цепочек `cmd.arg(...)`, порядок
//! флагов нигде не был зафиксирован, а он значащий: `-ss` до `-i` — это одно, после `-i` —
//! совсем другое. Здесь каждое задание описано структурой, превращается в аргументы одной
//! функцией, и порядок закреплён юнит-тестами.

use anyhow::{Context, Result};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const FFMPEG_BIN: &str = "ffmpeg";
pub const FFPROBE_BIN: &str = "ffprobe";

/// Общие для всех наших вызовов флаги: не спрашивать ничего у stdin и не сыпать баннером.
fn base_args() -> Vec<OsString> {
    vec![
        OsString::from("-nostdin"),
        OsString::from("-loglevel"),
        OsString::from("error"),
    ]
}

fn push(args: &mut Vec<OsString>, flag: &str, value: impl AsRef<OsStr>) {
    args.push(OsString::from(flag));
    args.push(value.as_ref().to_os_string());
}

/// Как держать перекодированный звук на таймстемпах источника.
#[derive(Clone, Copy)]
enum AudioSync {
    Off,
    Async,
    /// То же, плюс тишина от нуля до первого сэмпла. Только для рана с начала файла: у рана с
    /// середины под `-copyts` это означало бы тишину длиной во всю пропущенную часть фильма.
    AsyncFromZero,
}

/// Аргументы аудиокодека — одинаковы во всех заданиях, поэтому живут в одном месте:
/// расхождение между профилями дало бы разное звучание одного файла в зависимости от того,
/// каким путём он поехал.
fn audio_args(transcode: bool, sync: AudioSync) -> Vec<OsString> {
    let mut args = Vec::new();
    if transcode {
        push(&mut args, "-c:a", "aac");
        push(&mut args, "-b:a", "192k");
        push(&mut args, "-ac", "2");
        match sync {
            AudioSync::Off => {}
            AudioSync::Async => push(&mut args, "-af", "aresample=async=1"),
            AudioSync::AsyncFromZero => push(&mut args, "-af", "aresample=async=1:first_pts=0"),
        }
    } else {
        push(&mut args, "-c:a", "copy");
    }
    args
}

pub fn is_ffmpeg_available() -> bool {
    Command::new(FFMPEG_BIN).arg("-version").output().is_ok()
}

/// Готовая к запуску `tokio`-команда с нашими аргументами.
pub fn command(args: Vec<OsString>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(FFMPEG_BIN);
    cmd.args(args);
    cmd
}

/// Полные метаданные файла в JSON (`-show_format -show_streams`).
pub fn probe_format_json(file_path: &Path) -> Result<Value> {
    let output = Command::new(FFPROBE_BIN)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(file_path)
        .output()
        .context(
            "Не удалось запустить ffprobe. Убедитесь, что ffprobe установлен и доступен в PATH",
        )?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe завершился с ошибкой: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).context("Не удалось распарсить JSON вывод ffprobe")
}

/// Пакеты видеопотока в CSV `pts_time,dts_time,flags`.
///
/// Работа идёт по пакетам, а не по кадрам: это чтение индекса контейнера, без декодирования —
/// на порядки быстрее, чем `-skip_frame nokey`.
pub fn probe_video_packets_csv(file_path: &Path) -> Result<String> {
    let output = Command::new(FFPROBE_BIN)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,dts_time,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(file_path)
        .output()
        .context("Не удалось запустить ffprobe для индекса keyframes")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe завершился с ошибкой при чтении keyframes: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Промежуток между первым и последним видеокадром сегмента. Используется для проверки
/// инварианта «выданный сегмент длится ровно столько, сколько обещает карта».
///
/// Считаем именно по видеодорожке, а не по `format=duration`: длительность контейнера держит
/// аудио, поэтому сегмент с выброшенным видео по ней выглядит здоровым.
pub fn probe_video_span_seconds(file_path: &Path) -> Result<f64> {
    let output = Command::new(FFPROBE_BIN)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=dts_time",
            "-of",
            "csv=p=0",
        ])
        .arg(file_path)
        .output()
        .context("Не удалось запустить ffprobe для длительности")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe завершился с ошибкой при чтении длительности: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // csv=p=0 дописывает разделитель в конец каждой строки, кроме последней: без обрезки
    // разбирается ровно одна строка и span всегда выходит нулевым.
    let mut times = text
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse::<f64>().ok())
        .peekable();
    let first = times
        .peek()
        .copied()
        .context("в сегменте нет видеокадров")?;
    let last = times.fold(first, |_, t| t);
    Ok(last - first)
}

/// Видеопоток источника в той части, от которой зависит конвейер на GPU (что умеет
/// декодировать сам iGPU, нужен ли тонмаппинг) и copy HEVC клиенту, который его играет.
/// В БД не хранится — читается по заголовкам при запуске рана или выборе профиля.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceVideo {
    pub codec: String,
    pub pix_fmt: String,
    pub color_transfer: String,
    /// `dv_bl_signal_compatibility_id` из конфигурации Dolby Vision; `None` — DV нет.
    pub dv_bl_compat: Option<u8>,
}

impl SourceVideo {
    /// Dolby Vision без совместимого базового слоя (profile 5, IPTPQc2). Плеер без DV
    /// показывает такой поток зелёно-фиолетовым, поэтому копировать его нельзя.
    pub fn is_dolby_vision_only(&self) -> bool {
        self.dv_bl_compat == Some(0)
    }

    /// HDR10/Dolby Vision с HDR10-слоем (PQ) и HLG. Без тонмаппинга такой поток выходит из
    /// SDR-кодировщика блёклым: значения остаются в PQ/bt2020, а показываются как bt709.
    pub fn is_hdr(&self) -> bool {
        matches!(self.color_transfer.as_str(), "smpte2084" | "arib-std-b67")
    }

    fn is_10bit(&self) -> bool {
        self.pix_fmt.contains("10le") || self.pix_fmt.contains("10be")
    }

    /// Декодирует ли поток сам iGPU (Intel Gen9+, драйвер iHD). Остальное декодирует CPU
    /// и загружает кадры на GPU. Угадывать нельзя: `-hwaccel` на неподдержанном профиле
    /// молча откатывается на программный декодер, и кадры приходят в `scale_vaapi` из
    /// обычной памяти — ран падает.
    fn gpu_decodable(&self) -> bool {
        let yuv420 = matches!(
            self.pix_fmt.as_str(),
            "yuv420p" | "yuvj420p" | "nv12" | "yuv420p10le" | "p010le"
        );
        let codec = match self.codec.as_str() {
            "hevc" | "vp9" => true,
            // H.264 10-bit (Hi10P) аппаратного декодера нет ни у одного поколения Intel.
            "h264" | "mpeg2video" | "vc1" => !self.is_10bit(),
            _ => false,
        };
        yuv420 && codec
    }
}

/// `codec_name`, `pix_fmt` и `color_transfer` первого видеопотока. Читает заголовки
/// контейнера, без декодирования.
pub fn probe_source_video(file_path: &Path) -> Result<SourceVideo> {
    let output = Command::new(FFPROBE_BIN)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,pix_fmt,color_transfer:stream_side_data=dv_bl_signal_compatibility_id",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(file_path)
        .output()
        .context("Не удалось запустить ffprobe для видеопотока")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe завершился с ошибкой при чтении видеопотока: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut source = SourceVideo::default();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match line.split_once('=') {
            Some(("codec_name", v)) => source.codec = v.to_string(),
            Some(("pix_fmt", v)) => source.pix_fmt = v.to_string(),
            Some(("color_transfer", v)) => source.color_transfer = v.to_string(),
            Some(("dv_bl_signal_compatibility_id", v)) => source.dv_bl_compat = v.parse().ok(),
            _ => {}
        }
    }
    Ok(source)
}

/// Кодирует ли VAAPI на `device` H.264 на самом деле. Устройства мало: без драйвера
/// (`libva-intel-media-driver`) `/dev/dri` есть, а кодировать нечем.
pub fn vaapi_h264_works(device: &Path) -> bool {
    let mut args = base_args();
    push(&mut args, "-init_hw_device", vaapi_device_arg(device));
    push(&mut args, "-filter_hw_device", "va");
    push(&mut args, "-f", "lavfi");
    push(&mut args, "-i", "testsrc=size=320x240:rate=25:duration=0.2");
    push(&mut args, "-vf", "format=nv12,hwupload");
    push(&mut args, "-c:v", "h264_vaapi");
    push(&mut args, "-f", "null");
    args.push(OsString::from("-"));
    Command::new(FFMPEG_BIN)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn vaapi_device_arg(device: &Path) -> OsString {
    let mut arg = OsString::from("vaapi=va:");
    arg.push(device);
    arg
}

/// Чем кодируется видео в ране с перекодированием.
#[derive(Debug, Clone, Copy)]
pub enum VideoEncoder<'a> {
    /// libx264 на CPU. Источник не уменьшается, тонмаппинга нет: в сборке ffmpeg из pkg
    /// нет `zscale`, а без него HDR в SDR не перевести.
    Software,
    /// Декодирование, уменьшение до экрана клиента, тонмаппинг HDR и H.264 на iGPU. Замер на
    /// i7-9700T (UHD 630), 4K HEVC 10-bit HDR → 1080p: 5.2× реального времени при ~0.4 ядра
    /// против 1.36× при семи ядрах у libx264 в 4K. См. docs/decisions/0011.
    Vaapi {
        device: &'a Path,
        source: &'a SourceVideo,
        /// Потолок высоты кадра; ширина — 16:9 от него. 4K в H.264 на iGPU вдвое медленнее
        /// 1080p (2.8× против 6.0×), поэтому выше экрана клиента не кодируем.
        max_height: u32,
    },
}

/// Фильтры GPU-конвейера.
fn vaapi_filters(source: &SourceVideo, deinterlace: bool, max_height: u32) -> String {
    let mut chain = Vec::new();
    if !source.gpu_decodable() {
        // 10 бит грузим как есть: тонмаппингу нужен p010, а в 8 бит сведёт scale_vaapi.
        let format = if source.is_10bit() { "p010" } else { "nv12" };
        chain.push(format!("format={format},hwupload"));
    }
    if deinterlace {
        // rate=frame — кадр на пару полей, как yadif=mode=0 у CPU-конвейера.
        chain.push("deinterlace_vaapi=rate=frame".to_string());
    }
    let tonemap = source.is_hdr();
    // Уменьшаем до тонмаппинга: tonemap_vaapi на 4K-кадре вдвое медленнее, чем на 1080p
    // (замер: 2.5× против 5.2×). Поэтому scale оставляет 10 бит, если дальше тонмаппинг.
    // Запятые в min() экранированы — иначе граф фильтров разрежет по ним выражение.
    let max_width = max_height * 16 / 9;
    chain.push(format!(
        "scale_vaapi=w=min(iw\\,{max_width}):h=min(ih\\,{max_height})\
         :force_original_aspect_ratio=decrease:force_divisible_by=2:format={}",
        if tonemap { "p010" } else { "nv12" }
    ));
    if tonemap {
        chain.push("tonemap_vaapi=format=nv12:t=bt709:m=bt709:p=bt709".to_string());
    }
    chain.join(",")
}

/// Прогрессивная отдача fMP4 в stdout с перекодированием аудио — старый (не-HLS) путь
/// воспроизведения.
pub struct AudioTranscodeStream<'a> {
    pub input: &'a Path,
    /// Позиция seek в секундах; `-ss` идёт до `-i` — быстрый поиск по индексу.
    pub start_time: u64,
    /// Номер аудиодорожки среди аудиопотоков файла.
    pub audio_index: u32,
    pub duration: Option<u64>,
}

impl AudioTranscodeStream<'_> {
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args = base_args();
        if self.start_time > 0 {
            push(&mut args, "-ss", self.start_time.to_string());
        }
        push(&mut args, "-i", self.input);
        push(&mut args, "-map", "0:v:0");
        push(&mut args, "-map", format!("0:a:{}", self.audio_index));
        if let Some(dur) = self.duration {
            push(&mut args, "-t", dur.to_string());
        }
        push(&mut args, "-c:v", "copy");
        args.extend(audio_args(true, AudioSync::Off));
        push(&mut args, "-ar", "48000");
        // Синхронизирует аудио после seek, добавляя тишину в начале если нужно.
        push(&mut args, "-af", "aresample=async=1:first_pts=0");
        push(&mut args, "-f", "mp4");
        push(
            &mut args,
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof+faststart",
        );
        // Отрицательные таймстемпы после seek ломают воспроизведение с начала потока.
        push(&mut args, "-avoid_negative_ts", "make_zero");
        args.push(OsString::from("-"));
        args
    }
}

/// Один HLS-ран с перекодированием видео: границы сегментов задаём сами через
/// `-force_key_frames`, поэтому сетка hls-муксера (`start + n*segment_seconds`) с ними совпадает
/// и можно доверить нарезку муксеру.
pub struct HlsEncodeRun<'a> {
    pub input: &'a Path,
    pub dir: &'a Path,
    pub start_time: f64,
    pub start_index: usize,
    pub transcode_audio: bool,
    /// Собрать кадры из полей чересстрочного источника. `mode=0` даёт кадр на пару полей
    /// (25 fps из 25 кадров), а не удвоение частоты: материал SD-эфира, платить вдвое
    /// большим декодом на телевизоре за это не стоит.
    pub deinterlace: bool,
    /// Ограничение длительности рана. Задаётся только для init-рана, который стартует
    /// с нуля файла: под `-copyts -start_at_zero` `-t` отсчитывается по исходной шкале
    /// таймстемпов, поэтому для рана с середины он обрезал бы выход почти сразу
    /// (замер: 1 пакет вместо 250), а `h264_vaapi` на таком пустом выходе падает с
    /// segfault. См. docs/decisions/0005 и 0011.
    pub max_seconds: Option<f64>,
    pub segment_seconds: f64,
    pub init_name: &'a str,
    /// Номер аудиодорожки среди аудиопотоков файла.
    pub audio_index: u32,
    pub encoder: VideoEncoder<'a>,
}

impl HlsEncodeRun<'_> {
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args = base_args();
        if let VideoEncoder::Vaapi { device, source, .. } = self.encoder {
            push(&mut args, "-init_hw_device", vaapi_device_arg(device));
            push(&mut args, "-filter_hw_device", "va");
            if source.gpu_decodable() {
                push(&mut args, "-hwaccel", "vaapi");
                push(&mut args, "-hwaccel_device", "va");
                push(&mut args, "-hwaccel_output_format", "vaapi");
            }
        }
        if self.start_time > 0.0 {
            push(&mut args, "-ss", format!("{:.6}", self.start_time));
        }
        push(&mut args, "-i", self.input);
        push(&mut args, "-map", "0:v:0");
        push(&mut args, "-map", format!("0:a:{}", self.audio_index));

        // Фильтр идёт до кодека: yadif работает над декодированными кадрами.
        match self.encoder {
            VideoEncoder::Software => {
                if self.deinterlace {
                    push(&mut args, "-vf", "yadif=mode=0");
                }
                push(&mut args, "-c:v", "libx264");
                push(&mut args, "-preset", "veryfast");
                push(&mut args, "-crf", "23");
                push(&mut args, "-pix_fmt", "yuv420p");
            }
            VideoEncoder::Vaapi {
                source, max_height, ..
            } => {
                push(
                    &mut args,
                    "-vf",
                    vaapi_filters(source, self.deinterlace, max_height),
                );
                push(&mut args, "-c:v", "h264_vaapi");
                push(&mut args, "-qp", "23");
                // -pix_fmt здесь нельзя: кадры живут в памяти GPU, формат задал scale_vaapi.
            }
        }
        // `t` в этом выражении отсчитывается от начала рана, а не от начала файла: `-copyts`
        // влияет на записываемые таймстемпы, но `-start_at_zero` держит внутреннее время
        // относительным. Сдвиг на start_time (казавшийся очевидным) делал выражение всегда
        // ложным — x264 резал по собственному GOP, сегменты уезжали на 2–3 c от карты.
        // Замер на реальном файле: с shift 8.4/4.1/7.8/8.8 c против обещанных 6.0; без — 6.0.
        push(
            &mut args,
            "-force_key_frames",
            format!("expr:gte(t,n_forced*{})", self.segment_seconds),
        );

        // Без тишины в начале у файла с поздним звуком буфер плеера начинался бы с дыры.
        let sync = if self.start_time == 0.0 {
            AudioSync::AsyncFromZero
        } else {
            AudioSync::Async
        };
        args.extend(audio_args(self.transcode_audio, sync));

        // Абсолютные таймстемпы каждого рана должны совпадать с картой сегментов — иначе плеер
        // положит сегмент не туда в буфере. НЕ ставить -avoid_negative_ts make_zero: он обнулил
        // бы таймлайн рана, стартующего не с нуля файла.
        args.push(OsString::from("-copyts"));
        args.push(OsString::from("-start_at_zero"));
        push(&mut args, "-muxdelay", "0");

        if let Some(seconds) = self.max_seconds {
            debug_assert!(
                self.start_time == 0.0,
                "-t под -copyts корректен только для рана с начала файла"
            );
            push(&mut args, "-t", format!("{seconds:.3}"));
        }

        push(&mut args, "-f", "hls");
        push(&mut args, "-hls_time", format!("{}", self.segment_seconds));
        push(&mut args, "-hls_list_size", "0");
        // temp_file: ffmpeg пишет во временное имя и делает rename — читатель никогда не увидит
        // недописанный сегмент.
        push(&mut args, "-hls_flags", "temp_file");
        push(&mut args, "-hls_segment_type", "fmp4");
        // Без frag_discont muxer считает tfdt каждой дорожки от её первого пакета, а разницу
        // пишет в edit list init.mp4. hls.js edit list не читает, поэтому поздний звук играл бы
        // раньше картинки, а init зависел бы от рана. См. docs/decisions/0013.
        push(&mut args, "-hls_segment_options", "movflags=+frag_discont");
        push(&mut args, "-hls_fmp4_init_filename", self.init_name);
        push(
            &mut args,
            "-hls_segment_filename",
            self.dir.join("segment%05d.m4s"),
        );
        push(&mut args, "-start_number", self.start_index.to_string());
        args.push(OsString::from(
            self.dir.join(format!("run.{}.m3u8", self.start_index)),
        ));
        args
    }
}

/// Насколько раньше нужной границы ставится входной `-ss` при `-c:v copy`: быстрый seek по
/// индексу контейнера попадает на keyframe заметно раньше цели (в mkv — на границу кластера).
pub const INPUT_SEEK_SLACK: f64 = 20.0;

/// Один сегмент при `-c:v copy`.
///
/// Резать можно только по keyframe исходника, и ни один муксер не режет ровно по заданному
/// списку точек, если ран стартует не с начала файла: `hls` идёт по собственному расписанию,
/// а `segment` считает `-segment_times` от старта рана, который после входного `-ss` уезжает
/// на более ранний keyframe. Поэтому границу задаёт не муксер, а сам вызов: приблизительный
/// вход (`-ss` до `-i`) + точный выходной `-ss` + `-t` на длину; абсолютные таймстемпы
/// возвращает `-output_ts_offset`. Проверено `scripts/hls-cut-probe.sh`.
pub struct CopySegment<'a> {
    pub input: &'a Path,
    pub from: f64,
    pub duration: f64,
    pub transcode_audio: bool,
    /// Номер аудиодорожки среди аудиопотоков файла.
    pub audio_index: u32,
    pub output: PathBuf,
}

impl CopySegment<'_> {
    pub fn to_args(&self) -> Vec<OsString> {
        // Запас перед границей: входной -ss попадает на keyframe не позже цели, дальше кадры
        // до границы отбрасывает выходной -ss.
        let slack = (self.from - INPUT_SEEK_SLACK).max(0.0);

        let mut args = base_args();
        if slack > 0.0 {
            push(&mut args, "-ss", format!("{slack:.6}"));
        }
        push(&mut args, "-i", self.input);
        // Выходной `-ss 0` — не no-op: у mkv старт видеодорожки не совпадает с нулём, и ffmpeg
        // выбрасывает из копии всё видео, оставляя один кадр. Нулевой сдвиг не пишем вовсе.
        let exact = self.from - slack;
        if exact > 0.0 {
            push(&mut args, "-ss", format!("{exact:.6}"));
        }
        push(&mut args, "-t", format!("{:.6}", self.duration));
        push(&mut args, "-output_ts_offset", format!("{:.6}", self.from));
        push(&mut args, "-map", "0:v:0");
        push(&mut args, "-map", format!("0:a:{}", self.audio_index));
        push(&mut args, "-c:v", "copy");
        args.extend(audio_args(self.transcode_audio, AudioSync::Async));
        push(&mut args, "-muxdelay", "0");
        push(&mut args, "-muxpreload", "0");
        push(&mut args, "-f", "mpegts");
        args.push(self.output.clone().into_os_string());
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn index_of(args: &[String], needle: &str) -> usize {
        args.iter().position(|a| a == needle).expect(needle)
    }

    #[test]
    fn copy_segment_puts_slack_seek_before_input_and_exact_seek_after() {
        let args = strs(
            &CopySegment {
                input: Path::new("/m/f.mkv"),
                from: 100.0,
                duration: 6.0,
                transcode_audio: true,
                audio_index: 0,
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );

        let i = index_of(&args, "-i");
        let seeks: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "-ss")
            .map(|(n, _)| n)
            .collect();

        // Два -ss: грубый до -i (100 - slack) и точный после -i (остаток до границы).
        assert_eq!(seeks.len(), 2, "{args:?}");
        assert!(seeks[0] < i && seeks[1] > i, "{args:?}");
        assert_eq!(
            args[seeks[0] + 1],
            format!("{:.6}", 100.0 - INPUT_SEEK_SLACK)
        );
        assert_eq!(args[seeks[1] + 1], format!("{INPUT_SEEK_SLACK:.6}"));
        // Абсолютное время сегмента на выходе — иначе плеер положит его не туда в буфере.
        assert_eq!(args[index_of(&args, "-output_ts_offset") + 1], "100.000000");
        assert_eq!(args[index_of(&args, "-t") + 1], "6.000000");
        assert_eq!(args[index_of(&args, "-c:v") + 1], "copy");
    }

    #[test]
    fn copy_segment_near_start_has_single_seek() {
        let args = strs(
            &CopySegment {
                input: Path::new("/m/f.mkv"),
                from: 4.0,
                duration: 6.0,
                transcode_audio: false,
                audio_index: 0,
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        // Запас упирается в начало файла — грубого seek нет, точный отсчитывается от нуля.
        assert_eq!(args.iter().filter(|a| a.as_str() == "-ss").count(), 1);
        let ss = index_of(&args, "-ss");
        assert!(ss > index_of(&args, "-i"));
        assert_eq!(args[ss + 1], "4.000000");
        assert_eq!(args[index_of(&args, "-c:a") + 1], "copy");
    }

    #[test]
    fn copy_segment_from_file_start_has_no_seek_at_all() {
        let args = strs(
            &CopySegment {
                input: Path::new("/m/f.mkv"),
                from: 0.0,
                duration: 6.0,
                transcode_audio: true,
                audio_index: 0,
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        // `-ss 0` выкидывает видео из copy-сегмента целиком — нулевого seek быть не должно.
        assert!(!args.iter().any(|a| a == "-ss"), "{args:?}");
        assert_eq!(args[index_of(&args, "-t") + 1], "6.000000");
    }

    /// Порядок значим: -vf относится к декодированному входу и обязан стоять до -c:v,
    /// иначе фильтр применится не к тому этапу и yadif молча не сработает.
    #[test]
    fn deinterlace_adds_yadif_before_encoder() {
        let args = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.ts"),
                dir: Path::new("/tmp/c"),
                start_time: 0.0,
                start_index: 0,
                transcode_audio: true,
                deinterlace: true,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.0.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );

        assert_eq!(args[index_of(&args, "-vf") + 1], "yadif=mode=0");
        assert!(index_of(&args, "-vf") < index_of(&args, "-c:v"));
    }

    /// Прогрессивный источник не должен платить за фильтр.
    #[test]
    fn progressive_run_has_no_filter() {
        let args = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 0.0,
                start_index: 0,
                transcode_audio: true,
                deinterlace: false,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.0.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );

        assert!(!args.contains(&"-vf".to_string()));
    }

    #[test]
    fn encode_run_keeps_absolute_timestamps() {
        let args = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 60.0,
                start_index: 10,
                transcode_audio: true,
                deinterlace: false,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.10.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );

        assert!(args.contains(&"-copyts".to_string()));
        assert!(args.contains(&"-start_at_zero".to_string()));
        // make_zero обнулил бы таймлайн рана, стартующего не с начала файла.
        assert!(!args.contains(&"-avoid_negative_ts".to_string()));
        // Единственный -ss — до -i.
        assert!(index_of(&args, "-ss") < index_of(&args, "-i"));
        // Выражение НЕ сдвигается на start_time: под -start_at_zero t относительно рана.
        assert_eq!(
            args[index_of(&args, "-force_key_frames") + 1],
            "expr:gte(t,n_forced*6)"
        );
        assert_eq!(args[index_of(&args, "-start_number") + 1], "10");
        assert_eq!(args[index_of(&args, "-hls_time") + 1], "6");
    }

    #[test]
    fn encode_run_from_file_start_has_no_seek() {
        let args = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 0.0,
                start_index: 0,
                transcode_audio: false,
                deinterlace: false,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.0.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );
        assert!(!args.contains(&"-ss".to_string()));
        assert_eq!(args[index_of(&args, "-c:a") + 1], "copy");
    }

    #[test]
    fn audio_recipe_is_identical_across_jobs() {
        let copy = strs(
            &CopySegment {
                input: Path::new("/m/f.mkv"),
                from: 0.0,
                duration: 6.0,
                transcode_audio: true,
                audio_index: 0,
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        let encode = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 60.0,
                start_index: 10,
                transcode_audio: true,
                deinterlace: false,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.10.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );

        for flag in ["-c:a", "-b:a", "-ac", "-af"] {
            assert_eq!(
                copy[index_of(&copy, flag) + 1],
                encode[index_of(&encode, flag) + 1],
                "аудио-рецепт разошёлся на {flag}"
            );
        }
    }

    /// Тишина до первого сэмпла нужна только там, где ран начинается с нуля файла.
    #[test]
    fn only_run_from_file_start_pads_audio_from_zero() {
        let af = |start_time| {
            let args = strs(
                &HlsEncodeRun {
                    input: Path::new("/m/f.mkv"),
                    dir: Path::new("/tmp/c"),
                    start_time,
                    start_index: 0,
                    transcode_audio: true,
                    deinterlace: false,
                    max_seconds: None,
                    segment_seconds: 6.0,
                    init_name: "init.0.tmp.mp4",
                    audio_index: 0,
                    encoder: VideoEncoder::Software,
                }
                .to_args(),
            );
            args[index_of(&args, "-af") + 1].clone()
        };
        assert_eq!(af(0.0), "aresample=async=1:first_pts=0");
        assert_eq!(af(60.0), "aresample=async=1");
    }

    #[test]
    fn stream_job_seeks_before_input() {
        let args = strs(
            &AudioTranscodeStream {
                input: Path::new("/m/f.mkv"),
                start_time: 30,
                audio_index: 0,
                duration: Some(120),
            }
            .to_args(),
        );
        assert!(index_of(&args, "-ss") < index_of(&args, "-i"));
        assert_eq!(args[index_of(&args, "-t") + 1], "120");
        assert_eq!(args.last().unwrap(), "-");
    }

    fn vaapi_run_args(source: &SourceVideo, deinterlace: bool) -> Vec<String> {
        strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 60.0,
                start_index: 10,
                transcode_audio: true,
                deinterlace,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.10.tmp.mp4",
                audio_index: 0,
                encoder: VideoEncoder::Vaapi {
                    device: Path::new("/dev/dri/renderD128"),
                    source,
                    max_height: 1080,
                },
            }
            .to_args(),
        )
    }

    fn hdr_hevc() -> SourceVideo {
        SourceVideo {
            codec: "hevc".into(),
            pix_fmt: "yuv420p10le".into(),
            color_transfer: "smpte2084".into(),
            dv_bl_compat: None,
        }
    }

    /// Аппаратный декодер — входная опция: после -i она относилась бы к выходу и молча
    /// не сработала бы. Тонмаппинг идёт после уменьшения: на 4K-кадре он вдвое медленнее.
    #[test]
    fn vaapi_hdr_run_decodes_on_gpu_and_tonemaps_after_scale() {
        let args = vaapi_run_args(&hdr_hevc(), false);

        assert!(index_of(&args, "-hwaccel") < index_of(&args, "-i"));
        assert!(index_of(&args, "-init_hw_device") < index_of(&args, "-hwaccel"));
        let vf = &args[index_of(&args, "-vf") + 1];
        let scale = vf.find("scale_vaapi").expect(vf);
        let tonemap = vf.find("tonemap_vaapi").expect(vf);
        assert!(scale < tonemap, "{vf}");
        assert!(vf[scale..tonemap].contains("format=p010"), "{vf}");
        assert!(!vf.contains("hwupload"), "кадры уже на GPU: {vf}");
        assert_eq!(args[index_of(&args, "-c:v") + 1], "h264_vaapi");
        assert!(!args.contains(&"-pix_fmt".to_string()));
        // Сетка сегментов та же, что у libx264: карта от кодировщика не зависит.
        assert_eq!(
            args[index_of(&args, "-force_key_frames") + 1],
            "expr:gte(t,n_forced*6)"
        );
    }

    #[test]
    fn vaapi_sdr_run_has_no_tonemap() {
        let source = SourceVideo {
            codec: "hevc".into(),
            pix_fmt: "yuv420p10le".into(),
            color_transfer: "bt709".into(),
            dv_bl_compat: None,
        };
        let args = vaapi_run_args(&source, false);
        let vf = &args[index_of(&args, "-vf") + 1];
        assert!(!vf.contains("tonemap"), "{vf}");
        assert!(vf.contains("format=nv12"), "{vf}");
    }

    /// Потолок кадра — экран клиента, а не константа: 4K-экран без HEVC получает 4K.
    #[test]
    fn vaapi_scale_follows_client_height() {
        let vf = vaapi_filters(&hdr_hevc(), false, 2160);
        assert!(vf.contains("w=min(iw\\,3840):h=min(ih\\,2160)"), "{vf}");
        let vf = vaapi_filters(&hdr_hevc(), false, 720);
        assert!(vf.contains("w=min(iw\\,1280):h=min(ih\\,720)"), "{vf}");
    }

    /// mpeg4 iGPU не декодирует: без -hwaccel, кадры загружаются на GPU первым фильтром.
    #[test]
    fn vaapi_run_uploads_frames_gpu_cannot_decode() {
        let source = SourceVideo {
            codec: "mpeg4".into(),
            pix_fmt: "yuv420p".into(),
            color_transfer: String::new(),
            dv_bl_compat: None,
        };
        let args = vaapi_run_args(&source, true);
        assert!(!args.contains(&"-hwaccel".to_string()), "{args:?}");
        let vf = &args[index_of(&args, "-vf") + 1];
        assert!(vf.starts_with("format=nv12,hwupload,"), "{vf}");
        assert!(
            vf.find("deinterlace_vaapi").unwrap() < vf.find("scale_vaapi").unwrap(),
            "{vf}"
        );
    }

    #[test]
    fn gpu_decodes_neither_hi10p_nor_444() {
        let src = |codec: &str, pix_fmt: &str| SourceVideo {
            codec: codec.into(),
            pix_fmt: pix_fmt.into(),
            color_transfer: String::new(),
            dv_bl_compat: None,
        };
        assert!(src("hevc", "yuv420p10le").gpu_decodable());
        assert!(src("h264", "yuv420p").gpu_decodable());
        assert!(!src("h264", "yuv420p10le").gpu_decodable());
        assert!(!src("hevc", "yuv444p").gpu_decodable());
        assert!(!src("av1", "yuv420p10le").gpu_decodable());
    }
}

#[cfg(test)]
mod audio_track_tests {
    use super::*;

    #[test]
    fn recipes_map_requested_audio_track() {
        let maps = |args: Vec<OsString>| -> Vec<String> {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            args.windows(2)
                .filter(|w| w[0] == "-map")
                .map(|w| w[1].clone())
                .collect()
        };

        let copy = maps(
            CopySegment {
                input: Path::new("/m/f.mkv"),
                from: 0.0,
                duration: 6.0,
                transcode_audio: true,
                audio_index: 7,
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        assert_eq!(copy, vec!["0:v:0".to_string(), "0:a:7".to_string()]);

        let encode = maps(
            HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp"),
                start_time: 0.0,
                start_index: 0,
                transcode_audio: true,
                deinterlace: false,
                max_seconds: None,
                segment_seconds: 6.0,
                init_name: "init.mp4",
                audio_index: 3,
                encoder: VideoEncoder::Software,
            }
            .to_args(),
        );
        assert_eq!(encode, vec!["0:v:0".to_string(), "0:a:3".to_string()]);

        let stream = maps(
            AudioTranscodeStream {
                input: Path::new("/m/f.mkv"),
                start_time: 0,
                audio_index: 2,
                duration: None,
            }
            .to_args(),
        );
        assert_eq!(stream, vec!["0:v:0".to_string(), "0:a:2".to_string()]);
    }
}
