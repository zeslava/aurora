//! Оси, по которым различаются реальные медиафайлы, и общий чеклист проигрываемости.
//!
//! Зачем отдельный модуль: два дорогих бага подряд (рассинхрон сегментов и потеря видео в
//! нулевом сегменте) прошли мимо зелёных тестов по одной причине — фикстура была *здоровее*
//! реального релиза. `testsrc + libx264` без B-кадров даёт поток, у которого все пакеты имеют
//! DTS и ровный GOP; на нём неправильная нарезка неотличима от правильной.
//!
//! Поэтому контент попадает в тесты не файлом, а набором осей (см. [`Shape`]). Новая поломка
//! означает новую ось в матрице, а не правку под конкретный сериал.
//!
//! Замеры реальной библиотеки, из которых взяты оси:
//!
//! | | контейнер | видео | таймстемпы | GOP |
//! |---|---|---|---|---|
//! | Breaking Bad (LostFilm 1080p) | mkv | h264 | 2 пакета с `dts=N/A` | рваный, 1.2–10.4 c |
//! | Gossip Girl (WEB-DL 720p) | mkv | h264 | 1 пакет с `dts=N/A` | частый, 0.04–1.0 c |
//! | Scooby Doo (DVDRip) | avi | mpeg4 | `pts` нет вовсе | рваный, 0.9–10.4 c |

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Форма таймстемпов исходника. Ось всплыла на `-ss 0`: у mkv с B-пирамидой первые пакеты
/// приходят без DTS, и выходной seek выбрасывал всё видео до второго keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampForm {
    /// Все пакеты с DTS, PTS == DTS. Так выглядит фикстура «по умолчанию» — и почти никогда
    /// не выглядит релиз.
    Clean,
    /// Первые пакеты с `dts=N/A` (mkv + B-пирамида). Breaking Bad, Gossip Girl.
    NaDtsAtStart,
    /// PTS нет вообще, только DTS (avi). Scooby Doo.
    NoPts,
}

/// Форма GOP. Ось всплыла на первом баге: при равномерном GOP нарезка по карте и нарезка
/// hls-муксером совпадают случайно, и рассинхрон не воспроизводится.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GopForm {
    /// Ровно каждые 2 c — «удобные» данные, на которых проходит и неправильная нарезка.
    Uniform,
    /// Разброс от 1 до 10 c, как у эфирных рипов.
    Ragged,
    /// Keyframes через ~0.4 c, включая пару с зазором в один кадр: проверяет схлопывание
    /// границ в `collapse_keyframes`.
    Dense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoForm {
    /// Доживает до `-c:v copy`.
    H264,
    /// В MSE не декодируется — принудительное перекодирование видео, другая ветка нарезки.
    Mpeg4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioForm {
    /// Браузерный — аудио копируется.
    Aac,
    Ac3,
    Mp3,
}

impl AudioForm {
    pub fn codec(self) -> &'static str {
        match self {
            AudioForm::Aac => "aac",
            AudioForm::Ac3 => "ac3",
            AudioForm::Mp3 => "libmp3lame",
        }
    }

    /// Как этот кодек называет ffprobe — именно это значение лежит в БД.
    pub fn probed_name(self) -> &'static str {
        match self {
            AudioForm::Aac => "aac",
            AudioForm::Ac3 => "ac3",
            AudioForm::Mp3 => "mp3",
        }
    }
}

/// Точка в пространстве осей. Имя используется в сообщениях об ошибках, чтобы упавший
/// матричный тест сразу показывал, какая комбинация сломалась.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub name: &'static str,
    pub video: VideoForm,
    pub audio: AudioForm,
    pub timestamps: TimestampForm,
    pub gop: GopForm,
    pub seconds: u32,
}

impl Shape {
    /// Контейнер выводится из формы таймстемпов: `NoPts` бывает только у avi.
    pub fn extension(&self) -> &'static str {
        match self.timestamps {
            TimestampForm::NoPts => "avi",
            _ => "mkv",
        }
    }

    /// Список точек реза для `-force_key_frames`.
    fn key_frames(&self) -> String {
        match self.gop {
            GopForm::Uniform => "expr:gte(t,n_forced*2)".to_string(),
            GopForm::Ragged => {
                // Неравномерность принципиальна: на ровной сетке правильная нарезка
                // неотличима от нарезки муксером по собственному расписанию.
                let mut points = vec![0.0_f64, 4.0, 9.5, 14.5, 17.0, 19.5, 22.0, 25.5];
                points.retain(|t| *t < self.seconds as f64);
                points
                    .iter()
                    .map(|t| format!("{t}"))
                    .collect::<Vec<_>>()
                    .join(",")
            }
            GopForm::Dense => {
                // ~0.4 c плюс одна пара с зазором в кадр (0.04 c при 25 fps).
                let mut points: Vec<f64> = Vec::new();
                let mut t = 0.0;
                while t < self.seconds as f64 {
                    points.push(t);
                    t += 0.4;
                }
                points.push(0.44);
                points.sort_by(|a, b| a.partial_cmp(b).unwrap());
                points
                    .iter()
                    .map(|t| format!("{t:.2}"))
                    .collect::<Vec<_>>()
                    .join(",")
            }
        }
    }

    /// Генерирует файл этой формы. Реальные файлы в репозитории не нужны — все три формы
    /// таймстемпов воспроизводятся синтетикой.
    pub fn build(&self, dir: &Path) -> PathBuf {
        let path = dir.join(format!("{}.{}", self.name, self.extension()));
        let seconds = self.seconds;

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-y", "-loglevel", "error"]);
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={seconds}:size=320x240:rate=25"),
        ]);
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:duration={seconds}"),
        ]);

        match self.video {
            VideoForm::H264 => {
                cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
                match self.timestamps {
                    // B-пирамида — то, из-за чего у релизных mkv первые пакеты без DTS.
                    TimestampForm::NaDtsAtStart => {
                        cmd.args(["-bf", "3", "-x264opts", "b-pyramid=normal"]);
                    }
                    _ => {
                        cmd.args(["-bf", "0"]);
                    }
                }
            }
            VideoForm::Mpeg4 => {
                // Без B-кадров avi-мукссер проставляет pts всем пакетам, и форма `NoPts`
                // вырождается в `Clean`.
                // `-g` большой, иначе mpeg4 ставит свои keyframes каждые 12 кадров поверх
                // `-force_key_frames`, и заданная форма GOP не получается.
                cmd.args(["-c:v", "mpeg4", "-pix_fmt", "yuv420p", "-q:v", "5"]);
                cmd.args(["-bf", "2", "-g", "10000"]);
            }
        }

        cmd.args([
            "-sc_threshold",
            "0",
            "-force_key_frames",
            &self.key_frames(),
        ]);
        cmd.args(["-c:a", self.audio.codec()]);
        cmd.arg("-shortest").arg(&path);

        let status = cmd.status().expect("не удалось запустить ffmpeg");
        assert!(status.success(), "ffmpeg не собрал фикстуру {}", self.name);

        self.assert_shape_holds(&path);
        path
    }

    /// Проверяет, что сгенерированный файл действительно обладает заявленной формой.
    ///
    /// Без этой проверки матрица тихо вырождается: смена версии ffmpeg или флага — и все
    /// фикстуры снова становятся «чистыми», тесты остаются зелёными, покрытие исчезает.
    fn assert_shape_holds(&self, path: &Path) {
        let packets = probe_packets(path);
        assert!(!packets.is_empty(), "{}: нет видеопакетов", self.name);

        let na_dts = packets.iter().take(10).filter(|p| p.dts.is_none()).count();
        // У avi pts отсутствует ровно у первых пакетов — дальше демуксер его вычисляет.
        let no_pts = packets[0].pts.is_none();

        match self.timestamps {
            TimestampForm::Clean => assert!(
                na_dts == 0 && !no_pts,
                "{}: ожидались чистые таймстемпы, получено na_dts={na_dts} no_pts={no_pts}",
                self.name
            ),
            TimestampForm::NaDtsAtStart => assert!(
                na_dts > 0,
                "{}: ожидались пакеты с dts=N/A в начале, их нет — форма выродилась",
                self.name
            ),
            TimestampForm::NoPts => assert!(
                no_pts,
                "{}: ожидался контейнер без pts, а pts есть",
                self.name
            ),
        }

        let keys: Vec<f64> = packets
            .iter()
            .filter(|p| p.key)
            .filter_map(|p| p.time())
            .collect();
        assert!(keys.len() >= 3, "{}: слишком мало keyframes", self.name);
        let gaps: Vec<f64> = keys.windows(2).map(|w| w[1] - w[0]).collect();
        let min = gaps.iter().cloned().fold(f64::MAX, f64::min);
        let max = gaps.iter().cloned().fold(0.0_f64, f64::max);

        match self.gop {
            GopForm::Uniform => assert!(
                max - min < 0.5,
                "{}: ожидался ровный GOP, разброс {min:.2}..{max:.2}",
                self.name
            ),
            GopForm::Ragged => assert!(
                max - min > 2.0,
                "{}: ожидался рваный GOP, разброс {min:.2}..{max:.2}",
                self.name
            ),
            GopForm::Dense => assert!(
                max < 1.0 && min < 0.1,
                "{}: ожидался частый GOP с зазором в кадр, разброс {min:.2}..{max:.2}",
                self.name
            ),
        }
    }

    /// Матрица форм. Не декартово произведение: комбинации вроде «h264 в avi» в природе не
    /// встречаются и проверяли бы ffmpeg, а не нас.
    pub fn matrix() -> Vec<Shape> {
        vec![
            // Контрольная точка: «удобные» данные. Нужна, чтобы отличать поломку логики от
            // поломки на конкретной форме источника.
            Shape {
                name: "clean_h264_aac",
                video: VideoForm::H264,
                audio: AudioForm::Aac,
                timestamps: TimestampForm::Clean,
                gop: GopForm::Uniform,
                seconds: 30,
            },
            // Breaking Bad: copy-видео, рваный GOP, dts=N/A в начале.
            Shape {
                name: "lostfilm_h264_ac3",
                video: VideoForm::H264,
                audio: AudioForm::Ac3,
                timestamps: TimestampForm::NaDtsAtStart,
                gop: GopForm::Ragged,
                seconds: 30,
            },
            // Gossip Girl: то же, но keyframes через 0.4 c — граничный случай схлопывания.
            Shape {
                name: "webdl_h264_ac3_dense",
                video: VideoForm::H264,
                audio: AudioForm::Ac3,
                timestamps: TimestampForm::NaDtsAtStart,
                gop: GopForm::Dense,
                seconds: 30,
            },
            // Scooby Doo: avi без pts, mpeg4 → перекодирование видео, fMP4 + равномерная сетка.
            Shape {
                name: "dvdrip_mpeg4_mp3",
                video: VideoForm::Mpeg4,
                audio: AudioForm::Mp3,
                timestamps: TimestampForm::NoPts,
                gop: GopForm::Ragged,
                seconds: 30,
            },
            // Аудио уже браузерное при неудобном видео: единственная комбинация, где
            // перекодируется видео, но не звук.
            Shape {
                name: "dvdrip_mpeg4_aac",
                video: VideoForm::Mpeg4,
                audio: AudioForm::Aac,
                timestamps: TimestampForm::NoPts,
                gop: GopForm::Ragged,
                seconds: 30,
            },
        ]
    }
}

/// Один видеопакет в том виде, в каком его отдаёт ffprobe.
#[derive(Debug, Clone, Copy)]
pub struct Packet {
    pub pts: Option<f64>,
    pub dts: Option<f64>,
    pub key: bool,
}

impl Packet {
    /// Время пакета: dts, а при его отсутствии — pts. Тот же порядок, что в
    /// `segments::parse_keyframe_csv`, иначе тест и продакшн мерили бы разное.
    pub fn time(&self) -> Option<f64> {
        self.dts.or(self.pts)
    }
}

pub fn probe_packets(path: &Path) -> Vec<Packet> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "packet=pts_time,dts_time,flags"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("не удалось запустить ffprobe");
    assert!(
        out.status.success(),
        "ffprobe не прочитал {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().split(',');
            let pts = parts.next()?.parse::<f64>().ok();
            let dts = parts.next()?.parse::<f64>().ok();
            let flags = parts.next()?;
            Some(Packet {
                pts,
                dts,
                key: flags.starts_with('K'),
            })
        })
        .collect()
}

/// Параметры видеодорожки сегмента. `None` означает, что ffprobe не смог их прочитать —
/// то есть в сегменте нет SPS/PPS, и MSE такой сегмент не примет.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoParams {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub profile: Option<String>,
}

pub fn probe_video_params(path: &Path) -> VideoParams {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height,profile"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("не удалось запустить ffprobe");

    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.split(',');
    let profile = parts.next().map(str::to_string);
    let width = parts.next().and_then(|v| v.parse().ok());
    let height = parts.next().and_then(|v| v.parse().ok());
    VideoParams {
        width,
        height,
        // ffprobe пишет `unknown`, когда экстрадаты в сегменте нет.
        profile: profile.filter(|p| !p.is_empty() && p != "unknown"),
    }
}

/// Один сегмент так, как его увидит плеер: путь к файлу и обещанная плейлистом длительность.
pub struct SegmentUnderTest {
    pub name: String,
    pub path: PathBuf,
    pub extinf: f64,
}

/// Чеклист проигрываемости: то, что обязано быть верно для любой стратегии нарезки и любой
/// формы источника.
///
/// Проверки нарочно идут по видеодорожке, а не по `format=duration`: длительность контейнера
/// держит аудио, поэтому сегмент с выброшенным видео по ней выглядит здоровым — ровно так
/// баг с `-ss 0` и прошёл мимо тестов и мимо метрики `segment_drift`.
pub fn assert_playable(shape_name: &str, segments: &[SegmentUnderTest]) {
    assert!(
        segments.len() > 2,
        "{shape_name}: слишком мало сегментов ({})",
        segments.len()
    );

    let mut reference: Option<VideoParams> = None;
    let mut previous_end: Option<(String, f64)> = None;

    for seg in segments {
        let packets = probe_packets(&seg.path);

        // 1. Видео вообще есть. Плеер, получивший первый сегмент без видеодорожки, создаёт
        //    audio-only SourceBuffer, и на следующем сегменте падает с TrackRemovedError.
        assert!(
            !packets.is_empty(),
            "{shape_name}/{}: в сегменте нет видеопакетов",
            seg.name
        );

        // 2. SPS/PPS на месте — иначе MSE не соберёт декодер.
        let params = probe_video_params(&seg.path);
        assert!(
            params.profile.is_some() && params.width.is_some(),
            "{shape_name}/{}: нет параметров видео (SPS/PPS не попали в сегмент): {params:?}",
            seg.name
        );

        // 3. Параметры не меняются от сегмента к сегменту — смена разрешения на лету требует
        //    пересоздания SourceBuffer, чего плеер в рамках одного варианта не ждёт.
        match &reference {
            None => reference = Some(params),
            Some(want) => assert_eq!(
                want, &params,
                "{shape_name}/{}: параметры видео разъехались с первым сегментом",
                seg.name
            ),
        }

        let times: Vec<f64> = packets.iter().filter_map(|p| p.time()).collect();
        let first = *times.first().expect("время первого пакета");
        let last = times.iter().cloned().fold(first, f64::max);

        // 4. Длительность по видео совпадает с EXTINF. Допуск покрывает последний кадр,
        //    который в span не входит.
        let span = last - first;
        assert!(
            (span - seg.extinf).abs() < 0.5,
            "{shape_name}/{}: плейлист обещает {:.3} c, видео длится {span:.3} c",
            seg.name,
            seg.extinf
        );

        // 5. Стык с предыдущим сегментом: ни дыры, ни нахлёста. Именно накопление зазора
        //    давало подвисания каждые 5–10 c при формально успешных ffmpeg и HTTP 200.
        if let Some((prev_name, prev_end)) = &previous_end {
            let gap = first - prev_end;
            assert!(
                gap.abs() < 0.5,
                "{shape_name}: разрыв {gap:.3} c между {prev_name} (конец {prev_end:.3}) \
                 и {} (начало {first:.3})",
                seg.name
            );
        }
        previous_end = Some((seg.name.clone(), last));
    }
}
