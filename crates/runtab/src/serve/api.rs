use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::dto::{FilterParams, HeatmapResponse, SessionParams, SyncStatus};
use crate::ledger::{Ledger, ReviewItem, Settings};

#[derive(Clone)]
pub struct AppState {
    pub ledger: Arc<Mutex<Ledger>>,
}

impl AppState {
    pub(super) fn led(&self) -> Result<MutexGuard<'_, Ledger>, ApiError> {
        self.ledger
            .lock()
            .map_err(|_| ApiError::internal("ledger lock poisoned"))
    }
}

/// Uniform `{error, detail}` body for any non-2xx (contract "Error shape").
pub struct ApiError {
    status: StatusCode,
    error: String,
    detail: Option<String>,
}

impl ApiError {
    fn internal(detail: &str) -> ApiError {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "internal_error".to_string(),
            detail: Some(detail.to_string()),
        }
    }

    pub(super) fn bad_request(detail: &str) -> ApiError {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            error: "bad_request".to_string(),
            detail: Some(detail.to_string()),
        }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> ApiError {
        ApiError::internal(&e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.error, "detail": self.detail}))).into_response()
    }
}

pub(super) type Api<T> = Result<Json<T>, ApiError>;

pub async fn summary(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(st.led()?.api_summary(&q.filter())?))
}

pub async fn daily(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(json!({ "days": st.led()?.api_daily(&q.filter())? })))
}

pub async fn models(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(json!({ "models": st.led()?.api_models(&q.filter())? })))
}

pub async fn projects(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(json!({ "projects": st.led()?.api_projects(&q.filter())? })))
}

pub async fn agents(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(json!({ "agents": st.led()?.api_agents(&q.filter())? })))
}

pub async fn sessions(State(st): State<AppState>, Query(q): Query<SessionParams>) -> Api<impl serde::Serialize> {
    let filter = q.filter();
    let page = q.page.unwrap_or(1);
    let size = q.page_size.unwrap_or(50);
    Ok(Json(st.led()?.api_sessions(&filter, page, size)?))
}

pub async fn heatmap(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<HeatmapResponse> {
    let (days, max_tokens, horizon) = st.led()?.api_heatmap(&q.filter())?;
    Ok(Json(HeatmapResponse {
        days,
        max_tokens,
        deletion_horizon: Some(horizon),
    }))
}

pub async fn planwindow(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(st.led()?.api_planwindow(&q.filter())?))
}

pub async fn sync_status(State(st): State<AppState>) -> Api<SyncStatus> {
    let led = st.led()?;
    let s = led.sync_state()?;
    let state = if !s.enabled {
        "off"
    } else if s.degraded {
        "degraded"
    } else {
        "ok"
    };
    Ok(Json(SyncStatus {
        enabled: s.enabled,
        state: state.to_string(),
        account_email: s.account_email,
        server_seq: s.pull_cursor,
        pending_push: led.pending_push_count()?.max(0) as u64,
        last_push_at: s.last_push_at,
        last_pull_at: s.last_pull_at,
        message: s.message,
        machines: led.machine_stats()?,
    }))
}

/// Pre-sync review state: the projects that would sync and their rename/exclude
/// decisions. Labels are basenames — full paths never appear here or on the wire.
pub async fn get_review(State(st): State<AppState>) -> Api<impl serde::Serialize> {
    let led = st.led()?;
    Ok(Json(json!({
        "reviewed": led.projects_reviewed()?,
        "projects": led.project_review_items()?,
    })))
}

#[derive(serde::Deserialize)]
pub struct ReviewBody {
    pub projects: Vec<ReviewItem>,
}

/// Persist the review decisions (the dashboard's consent moment) so the push path
/// honours them. Enabling sync itself is still the browser device-authorization
/// flow (`runtab sync login`); this only records what may sync, never a bearer token.
pub async fn save_review(State(st): State<AppState>, Json(body): Json<ReviewBody>) -> Api<impl serde::Serialize> {
    let led = st.led()?;
    led.set_project_review(&body.projects)?;
    Ok(Json(json!({ "reviewed": true, "projects": led.project_review_items()? })))
}

/// One real derived record — exactly what a push would upload — for the
/// "See exactly what syncs" drawer. `null` when the ledger is empty.
pub async fn preview_record(State(st): State<AppState>) -> Api<impl serde::Serialize> {
    Ok(Json(json!({ "record": st.led()?.preview_record()? })))
}

pub async fn get_settings(State(st): State<AppState>) -> Api<Settings> {
    Ok(Json(st.led()?.settings()?))
}

pub async fn put_settings(State(st): State<AppState>, Json(body): Json<Settings>) -> Api<Settings> {
    Ok(Json(st.led()?.update_settings(&body)?))
}
