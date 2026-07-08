use axum::extract::{Query, State};
use axum::Json;

use super::api::{Api, AppState};
use super::dto::FilterParams;

/// `GET /api/savings?project=&machine=&from=&to=` — rtk's savings against the
/// real token consumption over the same global `Filter`. Attributed savings are
/// scoped through the `rtk_events.session_id` → `merged_events.session_id` join;
/// unattributed savings are surfaced separately (see `Ledger::savings`).
pub async fn savings(State(st): State<AppState>, Query(q): Query<FilterParams>) -> Api<impl serde::Serialize> {
    Ok(Json(st.led()?.savings(&q.filter())?))
}
