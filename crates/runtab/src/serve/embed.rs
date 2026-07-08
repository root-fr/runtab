use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

/// Serve an embedded SPA asset, falling back to `index.html` so client-side
/// routes resolve. Only compiled when the `embed-ui` feature is on.
pub async fn serve(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };
    if let Some(file) = Assets::get(path) {
        return ([(header::CONTENT_TYPE, content_type(path))], file.data).into_response();
    }
    match Assets::get("index.html") {
        Some(file) => ([(header::CONTENT_TYPE, content_type("index.html"))], file.data).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Content type by extension for the small set of asset kinds a Vite build
/// emits, so no mime-guessing dependency is needed.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}
