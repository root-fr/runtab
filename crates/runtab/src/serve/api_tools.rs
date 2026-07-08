use axum::extract::{Query, State};
use axum::Json;
use serde_json::json;

use super::api::{Api, ApiError, AppState};
use super::dto::ToolsParams;

/// `GET /api/tools?days=N&session=<id>` — per-tool-call token usage plus
/// rtk's own savings totals, both optionally scoped to a day count and/or a
/// session (see `Ledger::tool_aggregates`/`rtk_totals`).
pub async fn tools(State(st): State<AppState>, Query(q): Query<ToolsParams>) -> Api<impl serde::Serialize> {
    if q.days == Some(0) {
        return Err(ApiError::bad_request("days must be >= 1"));
    }
    let session = q.session();
    let led = st.led()?;
    let tools = led.tool_aggregates(q.days, session.as_deref())?;
    let rtk = led.rtk_totals(q.days, session.as_deref())?;
    Ok(Json(json!({ "tools": tools, "rtk": rtk })))
}
