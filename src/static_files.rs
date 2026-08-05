//! Раздача SPA. Ассеты вшиты в бинарь (`rust-embed`), поэтому рантайму не нужна
//! папка `frontend/dist` рядом с исполняемым файлом.
//!
//! `STATIC_DIR` остаётся как dev-override: если переменная задана и папка существует,
//! статика читается с диска (можно пересобирать фронт без пересборки бинаря).

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct Assets;

/// Fallback-хендлер: отдаёт запрошенный файл, иначе `index.html` (SPA-роутинг).
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    match Assets::get(path) {
        Some(file) => file_response(path, file),
        None => match Assets::get("index.html") {
            Some(index) => file_response("index.html", index),
            None => (StatusCode::NOT_FOUND, "Фронтенд не собран").into_response(),
        },
    }
}

fn file_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}
