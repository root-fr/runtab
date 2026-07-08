use axum::routing::get;
use axum::Router;

use super::api::{self, AppState};
use super::api_savings;
use super::api_tools;

/// The local dashboard router: `/api/*` over the merged SQLite, plus the SPA on
/// every other path.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/summary", get(api::summary))
        .route("/daily", get(api::daily))
        .route("/models", get(api::models))
        .route("/projects", get(api::projects))
        .route("/sessions", get(api::sessions))
        .route("/tools", get(api_tools::tools))
        .route("/savings", get(api_savings::savings))
        .route("/heatmap", get(api::heatmap))
        .route("/planwindow", get(api::planwindow))
        .route("/sync/status", get(api::sync_status))
        .route("/sync/review", get(api::get_review).post(api::save_review))
        .route("/sync/preview-record", get(api::preview_record))
        .route("/settings", get(api::get_settings).put(api::put_settings))
        .with_state(state);

    Router::new().nest("/api", api).fallback(ui_fallback)
}

#[cfg(feature = "embed-ui")]
async fn ui_fallback(uri: axum::http::Uri) -> axum::response::Response {
    super::embed::serve(uri).await
}

#[cfg(not(feature = "embed-ui"))]
async fn ui_fallback() -> axum::response::Html<&'static str> {
    axum::response::Html(
        "<!doctype html><title>runtab</title><body style=\"font-family:sans-serif;\
         background:#0a0a0b;color:#e5e5e7;padding:2rem\"><h1>runtab</h1>\
         <p>The dashboard UI was not embedded in this build. Rebuild with \
         <code>--features embed-ui</code> after <code>npm run build</code>. \
         The JSON API is live under <code>/api</code>.</p></body>",
    )
}
