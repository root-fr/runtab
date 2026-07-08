use rusqlite::params_from_iter;
use serde::Serialize;

use super::api_query::Filter;
use super::{schema::TOTAL_TOKENS_EXPR, Ledger};
use crate::billing::{self, Mode};
use crate::timeutil::{date_minus_days, epoch_to_rfc3339, now_epoch, parse_rfc3339_to_epoch, today_utc};

const H5: i64 = 5 * 3600;
const WEEK: i64 = 7 * 86_400;

#[derive(Debug, Serialize)]
pub struct SessionRow {
    pub session_id: String,
    pub project_label: String,
    pub machine_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub model: String,
    pub event_count: u64,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    /// rtk savings attributed to this session, joined by `session_id` alone
    /// (`merged_events` has no `source` column — see `query.rs`'s
    /// `SavedJoin::Session` for the two-column version the CLI report uses).
    /// `None` for a remote/synced session: its `session_id` is a sha256 hash
    /// (`push_rows.rs`), which never equals a local plain id, so it simply
    /// never matches — as well as for a local session rtk never attributed.
    pub rtk_saved_tokens: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SessionPage {
    pub sessions: Vec<SessionRow>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Serialize)]
pub struct Window {
    pub window_hours: u32,
    pub tokens_used: u64,
    pub limit_tokens: u64,
    pub utilization: f64,
    pub resets_at: String,
}

#[derive(Debug, Serialize)]
pub struct PlanUsage {
    pub tokens_used: u64,
    pub limit_tokens: u64,
    pub utilization: f64,
    pub resets_at: String,
}

#[derive(Debug, Serialize)]
pub struct PlanWindows {
    pub applicable: bool,
    pub rolling_5h: Option<Window>,
    pub weekly: Option<PlanUsage>,
    pub limits_are_estimates: bool,
}

fn u(v: i64) -> u64 {
    v.max(0) as u64
}

impl Ledger {
    pub fn api_sessions(&self, f: &Filter, page: u32, page_size: u32) -> rusqlite::Result<SessionPage> {
        let page = page.max(1);
        let page_size = page_size.clamp(1, 500);
        let (where_sql, p) = f.clause();

        let count_sql = format!(
            "SELECT COUNT(*) FROM (SELECT session_id FROM merged_events{where_sql} GROUP BY session_id)"
        );
        let total: i64 = self
            .conn
            .query_row(&count_sql, params_from_iter(p.iter()), |r| r.get(0))?;

        let sql = format!(
            "SELECT session_id, MAX(project_label), MAX(machine_name),
                    MIN(ts), MAX(ts), COUNT(*), COALESCE(SUM({total}),0),
                    COALESCE(SUM(est_cost_microusd),0),
                    COUNT(DISTINCT model), MAX(model),
                    (SELECT SUM(r.saved_tokens) FROM rtk_events r
                        WHERE r.session_id = merged_events.session_id)
             FROM merged_events{where_sql}
             GROUP BY session_id ORDER BY MIN(ts) DESC
             LIMIT {page_size} OFFSET {offset}",
            total = TOTAL_TOKENS_EXPR,
            // u64 math: a huge `page` query param must not overflow u32 (debug
            // panic / release wraparound). page >= 1 here, so the subtraction is safe.
            offset = (page as u64 - 1) * page_size as u64
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let sessions = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                let distinct: i64 = r.get(8)?;
                let one_model: String = r.get(9)?;
                Ok(SessionRow {
                    session_id: r.get(0)?,
                    project_label: r.get(1)?,
                    machine_name: r.get(2)?,
                    started_at: r.get(3)?,
                    ended_at: r.get(4)?,
                    model: if distinct > 1 { "mixed".to_string() } else { one_model },
                    event_count: u(r.get(5)?),
                    total_tokens: u(r.get(6)?),
                    est_cost_microusd: u(r.get(7)?),
                    rtk_saved_tokens: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(SessionPage { sessions, total: u(total), page, page_size })
    }

    pub fn api_planwindow(&self, f: &Filter) -> rusqlite::Result<PlanWindows> {
        let (sub_ev, api_ev) = self.mode_counts(f)?;
        let mode = billing::resolve(self.override_mode()?, sub_ev, api_ev);
        if mode == Mode::Api {
            return Ok(PlanWindows {
                applicable: false,
                rolling_5h: None,
                weekly: None,
                limits_are_estimates: true,
            });
        }

        // With an explicit subscription override, every event counts toward the
        // plan; otherwise only subscription-attributed (estimated) events do.
        let sub_only = self.override_mode()? != Some(Mode::Subscription);
        let (limit_5h, limit_week) = self.plan_limits()?;
        let events = self.recent_plan_tokens(f, sub_only)?;
        let now = now_epoch();

        Ok(PlanWindows {
            applicable: true,
            rolling_5h: Some(rolling(&events, now, H5, limit_5h)),
            weekly: Some(weekly(&events, now, limit_week)),
            limits_are_estimates: true,
        })
    }

    fn mode_counts(&self, f: &Filter) -> rusqlite::Result<(i64, i64)> {
        let (where_sql, p) = f.clause();
        let sql = format!(
            "SELECT SUM(CASE WHEN cost_basis='estimated' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN cost_basis IN ('logged','billed') THEN 1 ELSE 0 END)
             FROM merged_events{where_sql}"
        );
        self.conn.query_row(&sql, params_from_iter(p.iter()), |r| {
            Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0)))
        })
    }

    /// (ts_epoch, total_tokens) for events in the trailing 8 days — a bound big
    /// enough to cover both windows, so the in-Rust math works on a small set.
    fn recent_plan_tokens(&self, f: &Filter, sub_only: bool) -> rusqlite::Result<Vec<(i64, i64)>> {
        let (mut where_sql, mut p) = f.clause();
        if sub_only {
            where_sql.push_str(" AND cost_basis='estimated'");
        }
        p.push(date_minus_days(&today_utc(), 8));
        let sql = format!(
            "SELECT ts, {total} FROM merged_events{where_sql} AND substr(ts,1,10) >= ?",
            total = TOTAL_TOKENS_EXPR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .filter_map(|row| match row {
                Ok((ts, tok)) => parse_rfc3339_to_epoch(&ts).map(|e| Ok((e, tok))),
                Err(e) => Some(Err(e)),
            })
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

fn window_sum(events: &[(i64, i64)], now: i64, span: i64) -> (i64, i64) {
    let floor = now - span;
    let mut used = 0i64;
    let mut oldest = now;
    for &(ts, tok) in events {
        if ts >= floor && ts <= now {
            used += tok;
            oldest = oldest.min(ts);
        }
    }
    (used, oldest)
}

fn util(used: i64, limit: i64) -> f64 {
    if limit > 0 {
        used as f64 / limit as f64
    } else {
        0.0
    }
}

fn rolling(events: &[(i64, i64)], now: i64, span: i64, limit: i64) -> Window {
    let (used, oldest) = window_sum(events, now, span);
    Window {
        window_hours: (span / 3600) as u32,
        tokens_used: u(used),
        limit_tokens: u(limit),
        utilization: util(used, limit),
        resets_at: epoch_to_rfc3339(oldest + span),
    }
}

fn weekly(events: &[(i64, i64)], now: i64, limit: i64) -> PlanUsage {
    let (used, oldest) = window_sum(events, now, WEEK);
    PlanUsage {
        tokens_used: u(used),
        limit_tokens: u(limit),
        utilization: util(used, limit),
        resets_at: epoch_to_rfc3339(oldest + WEEK),
    }
}
