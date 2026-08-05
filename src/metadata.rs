use crate::ffmpeg;
use crate::models::MediaMetadata;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Извлекает метаданные из медиафайла с помощью ffprobe
pub fn extract_metadata(file_path: &Path) -> Result<MediaMetadata> {
    parse_ffprobe_output(&ffmpeg::probe_format_json(file_path)?)
}

/// Парсит JSON вывод ffprobe и извлекает метаданные
fn parse_ffprobe_output(json: &Value) -> Result<MediaMetadata> {
    let format = json
        .get("format")
        .context("Отсутствует секция 'format' в выводе ffprobe")?;
    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .context("Отсутствует секция 'streams' в выводе ffprobe")?;

    // Извлекаем длительность из format
    let duration = format
        .get("duration")
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| d as u64);

    // Находим видео и аудио потоки
    let mut video_stream = None;
    let mut audio_stream = None;

    for stream in streams {
        let codec_type = stream
            .get("codec_type")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        match codec_type {
            "video" if video_stream.is_none() => video_stream = Some(stream),
            "audio" if audio_stream.is_none() => audio_stream = Some(stream),
            _ => {}
        }
    }

    // Извлекаем данные из видео потока
    let (width, height, video_codec, video_bitrate, pix_fmt) = if let Some(video) = video_stream {
        let w = video
            .get("width")
            .and_then(|w| w.as_u64())
            .or_else(|| {
                video
                    .get("width")
                    .and_then(|w| w.as_str())
                    .and_then(|w| w.parse::<u64>().ok())
            })
            .map(|w| w as u32);

        let h = video
            .get("height")
            .and_then(|h| h.as_u64())
            .or_else(|| {
                video
                    .get("height")
                    .and_then(|h| h.as_str())
                    .and_then(|h| h.parse::<u64>().ok())
            })
            .map(|h| h as u32);

        let codec = video
            .get("codec_name")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        let bitrate = video
            .get("bit_rate")
            .and_then(|b| b.as_str())
            .and_then(|b| b.parse::<u64>().ok())
            .or_else(|| video.get("bit_rate").and_then(|b| b.as_u64()));

        let pix_fmt = video
            .get("pix_fmt")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());

        (w, h, codec, bitrate, pix_fmt)
    } else {
        (None, None, None, None, None)
    };

    // Извлекаем аудио кодек
    let audio_codec = audio_stream
        .and_then(|a| a.get("codec_name"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    // Если битрэйт не найден в видео потоке, пытаемся взять из format
    let video_bitrate = video_bitrate.or_else(|| {
        format
            .get("bit_rate")
            .and_then(|b| b.as_str())
            .and_then(|b| b.parse::<u64>().ok())
            .or_else(|| format.get("bit_rate").and_then(|b| b.as_u64()))
    });

    Ok(MediaMetadata {
        duration,
        width,
        height,
        video_codec,
        audio_codec,
        video_bitrate,
        pix_fmt,
    })
}
