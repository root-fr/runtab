//! Aggregate queries backing `runtab tools` and the dashboard's `/api/tools`:
//! token usage by tool-call type from `tool_events`, plus rtk's own
//! ledger-wide savings totals. Both take the same optional `(days, session)`
//! filters so the CLI (`None, None`) and the dashboard API compose from one
//! implementation.

use rusqlite::params_from_iter;
use serde::Serialize;

use super::Ledger;
use crate::timeutil::{date_minus_days, today_utc};

/// One aggregated row for the `runtab tools` report, largest `est_total_tokens`
/// first.
#[derive(Debug, Serialize)]
pub struct ToolAggregateRow {
    pub tool_name: String,
    pub calls: i64,
    pub est_args_tokens: i64,
    pub est_result_tokens: i64,
    pub est_total_tokens: i64,
    pub share_pct: f64,
}

/// rtk's own savings totals, optionally scoped by `(days, session)` — with no
/// filters it's every imported row, independent of attribution. `None` means
/// no row matched the filters (or rtk has never been imported), distinct
/// from a matched total of zero.
#[derive(Debug, Serialize)]
pub struct RtkTotals {
    pub commands: i64,
    pub saved_tokens: i64,
}

/// A ` WHERE …` fragment plus its bound values for `tool_events`/`rtk_events`,
/// both of which have a `ts` (completion timestamp, date-comparable via
/// `substr`) and a `session_id` column. `days` keeps the last N days
/// (inclusive), the same cutoff convention as `api_heatmap`'s 364-day window.
fn days_session_clause(days: Option<u32>, session: Option<&str>) -> (String, Vec<String>) {
    let mut sql = String::from(" WHERE 1=1");
    let mut p = Vec::new();
    if let Some(d) = days {
        sql.push_str(" AND substr(ts,1,10) >= ?");
        p.push(date_minus_days(&today_utc(), d as i64));
    }
    if let Some(s) = session {
        sql.push_str(" AND session_id = ?");
        p.push(s.to_string());
    }
    (sql, p)
}

impl Ledger {
    pub fn tool_aggregates(&self, days: Option<u32>, session: Option<&str>) -> rusqlite::Result<Vec<ToolAggregateRow>> {
        let (where_sql, p) = days_session_clause(days, session);
        let sql = format!(
            "SELECT tool_name, COUNT(*),
                    COALESCE(SUM(est_args_tokens), 0),
                    COALESCE(SUM(est_result_tokens), 0)
             FROM tool_events{where_sql}
             GROUP BY tool_name"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raw: Vec<(String, i64, i64, i64)> = stmt
            .query_map(params_from_iter(p.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?;

        let grand_total: i64 = raw.iter().map(|(_, _, args, result)| args + result).sum();
        let mut rows: Vec<ToolAggregateRow> = raw
            .into_iter()
            .map(|(tool_name, calls, est_args_tokens, est_result_tokens)| {
                let est_total_tokens = est_args_tokens + est_result_tokens;
                let share_pct = if grand_total > 0 {
                    100.0 * est_total_tokens as f64 / grand_total as f64
                } else {
                    0.0
                };
                ToolAggregateRow {
                    tool_name,
                    calls,
                    est_args_tokens,
                    est_result_tokens,
                    est_total_tokens,
                    share_pct,
                }
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.est_total_tokens));
        Ok(rows)
    }

    pub fn rtk_totals(&self, days: Option<u32>, session: Option<&str>) -> rusqlite::Result<Option<RtkTotals>> {
        let (where_sql, p) = days_session_clause(days, session);
        let sql = format!("SELECT COUNT(*), SUM(saved_tokens) FROM rtk_events{where_sql}");
        let (commands, saved_tokens): (i64, Option<i64>) = self
            .conn
            .query_row(&sql, params_from_iter(p.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(if commands == 0 {
            None
        } else {
            Some(RtkTotals { commands, saved_tokens: saved_tokens.unwrap_or(0) })
        })
    }
}
