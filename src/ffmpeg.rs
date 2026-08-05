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

/// Аргументы аудиокодека — одинаковы во всех заданиях, поэтому живут в одном месте:
/// расхождение между профилями дало бы разное звучание одного файла в зависимости от того,
/// каким путём он поехал.
fn audio_args(transcode: bool, resample_async: bool) -> Vec<OsString> {
    let mut args = Vec::new();
    if transcode {
        push(&mut args, "-c:a", "aac");
        push(&mut args, "-b:a", "192k");
        push(&mut args, "-ac", "2");
        if resample_async {
            push(&mut args, "-af", "aresample=async=1");
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

/// Прогрессивная отдача fMP4 в stdout с перекодированием аудио — старый (не-HLS) путь
/// воспроизведения.
pub struct AudioTranscodeStream<'a> {
    pub input: &'a Path,
    /// Позиция seek в секундах; `-ss` идёт до `-i` — быстрый поиск по индексу.
    pub start_time: u64,
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
        push(&mut args, "-map", "0:a:0");
        if let Some(dur) = self.duration {
            push(&mut args, "-t", dur.to_string());
        }
        push(&mut args, "-c:v", "copy");
        args.extend(audio_args(true, false));
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
        push(&mut args, "-reset_timestamps", "1");
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
    pub segment_seconds: f64,
    pub init_name: &'a str,
}

impl HlsEncodeRun<'_> {
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args = base_args();
        if self.start_time > 0.0 {
            push(&mut args, "-ss", format!("{:.6}", self.start_time));
        }
        push(&mut args, "-i", self.input);
        push(&mut args, "-map", "0:v:0");
        push(&mut args, "-map", "0:a:0");

        push(&mut args, "-c:v", "libx264");
        push(&mut args, "-preset", "veryfast");
        push(&mut args, "-crf", "23");
        push(&mut args, "-pix_fmt", "yuv420p");
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

        args.extend(audio_args(self.transcode_audio, true));

        // Абсолютные таймстемпы каждого рана должны совпадать с картой сегментов — иначе плеер
        // положит сегмент не туда в буфере. НЕ ставить -avoid_negative_ts make_zero: он обнулил
        // бы таймлайн рана, стартующего не с нуля файла.
        args.push(OsString::from("-copyts"));
        args.push(OsString::from("-start_at_zero"));
        push(&mut args, "-muxdelay", "0");

        push(&mut args, "-f", "hls");
        push(&mut args, "-hls_time", format!("{}", self.segment_seconds));
        push(&mut args, "-hls_list_size", "0");
        // temp_file: ffmpeg пишет во временное имя и делает rename — читатель никогда не увидит
        // недописанный сегмент.
        push(&mut args, "-hls_flags", "temp_file");
        push(&mut args, "-hls_segment_type", "fmp4");
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
        push(&mut args, "-map", "0:a:0");
        push(&mut args, "-c:v", "copy");
        args.extend(audio_args(self.transcode_audio, true));
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
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        // `-ss 0` выкидывает видео из copy-сегмента целиком — нулевого seek быть не должно.
        assert!(!args.iter().any(|a| a == "-ss"), "{args:?}");
        assert_eq!(args[index_of(&args, "-t") + 1], "6.000000");
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
                segment_seconds: 6.0,
                init_name: "init.10.tmp.mp4",
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
                segment_seconds: 6.0,
                init_name: "init.0.tmp.mp4",
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
                output: PathBuf::from("/tmp/seg.ts"),
            }
            .to_args(),
        );
        let encode = strs(
            &HlsEncodeRun {
                input: Path::new("/m/f.mkv"),
                dir: Path::new("/tmp/c"),
                start_time: 0.0,
                start_index: 0,
                transcode_audio: true,
                segment_seconds: 6.0,
                init_name: "init.0.tmp.mp4",
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

    #[test]
    fn stream_job_seeks_before_input() {
        let args = strs(
            &AudioTranscodeStream {
                input: Path::new("/m/f.mkv"),
                start_time: 30,
                duration: Some(120),
            }
            .to_args(),
        );
        assert!(index_of(&args, "-ss") < index_of(&args, "-i"));
        assert_eq!(args[index_of(&args, "-t") + 1], "120");
        assert_eq!(args.last().unwrap(), "-");
    }
}
