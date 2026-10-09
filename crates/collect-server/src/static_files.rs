//! 참여자 웹 정적 파일. 릴리스 빌드는 바이너리에 포함되고, 디버그 빌드는 디스크에서 바로 읽는다.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../apps/participant/dist"]
#[allow_missing = true]
struct Assets;

pub fn index() -> Response {
    match Assets::get("index.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
            Body::from(file.data),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "참여자 화면이 아직 빌드되지 않았습니다. `pnpm build:participant`를 실행하세요.",
        )
            .into_response(),
    }
}

pub async fn asset(Path(path): Path<String>) -> Response {
    // Vite가 해시를 붙인 파일명이라 오래 캐시해도 된다.
    serve(&format!("assets/{path}"), "public, max-age=31536000, immutable")
}

pub async fn fallback(uri: Uri) -> Response {
    serve(uri.path().trim_start_matches('/'), "no-cache")
}

/// `RustEmbed::get`은 포함된 파일 목록에서만 찾으므로 `../` 같은 경로로 디스크를 벗어날 수 없다.
fn serve(path: &str, cache: &'static str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref()), (header::CACHE_CONTROL, cache)], Body::from(file.data))
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
