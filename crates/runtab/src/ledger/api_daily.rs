use rusqlite::params_from_iter;
use serde::Serialize;

use super::api_query::Filter;
use super::{schema::TOTAL_TOKENS_EXPR, Ledger};
use crate::timeutil::{date_minus_days, today_utc};

#[derive(Debug, Serialize)]
pub struct DailyModel {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
}

#[derive(Debug, Serialize)]
pub struct DailyDay {
    pub date: String,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    pub by_model: Vec<DailyModel>,
}

#[derive(Debug, Serialize)]
pub struct HeatmapCell {
    pub date: String,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    pub level: u8,
}

fn u(v: i64) -> u64 {
    v.max(0) as u64
}

impl Ledger {
    pub fn api_daily(&self, f: &Filter) -> rusqlite::Result<Vec<DailyDay>> {
        let (where_sql, p) = f.clause();
        let sql = format!(
            "SELECT substr(ts,1,10) d, model,
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM({total}),0),
                    COALESCE(SUM(est_cost_microusd),0)
             FROM merged_events{where_sql} GROUP BY d, model ORDER BY d ASC, 8 DESC",
            total = TOTAL_TOKENS_EXPR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(p.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                DailyModel {
                    model: r.get(1)?,
                    input_tokens: u(r.get(2)?),
                    output_tokens: u(r.get(3)?),
                    cache_read_tokens: u(r.get(4)?),
                    cache_creation_tokens: u(r.get(5)?),
                    reasoning_tokens: u(r.get(6)?),
                    total_tokens: u(r.get(7)?),
                    est_cost_microusd: u(r.get(8)?),
                },
            ))
        })?;

        let mut days: Vec<DailyDay> = Vec::new();
        for row in rows {
            let (date, model) = row?;
            match days.last_mut() {
                Some(d) if d.date == date => {
                    d.total_tokens += model.total_tokens;
                    d.est_cost_microusd += model.est_cost_microusd;
                    d.by_model.push(model);
                }
                _ => days.push(DailyDay {
                    date,
                    total_tokens: model.total_tokens,
                    est_cost_microusd: model.est_cost_microusd,
                    by_model: vec![model],
                }),
            }
        }
        Ok(days)
    }

    /// 52-week contribution grid. Only days with activity are returned; the SPA
    /// fills the empty cells of the calendar.
    pub fn api_heatmap(&self, f: &Filter) -> rusqlite::Result<(Vec<HeatmapCell>, u64, String)> {
        let today = today_utc();
        let (where_sql, mut p) = f.clause();
        p.push(date_minus_days(&today, 364));
        let sql = format!(
            "SELECT substr(ts,1,10) d, COALESCE(SUM({total}),0), COALESCE(SUM(est_cost_microusd),0)
             FROM merged_events{where_sql} AND substr(ts,1,10) >= ?
             GROUP BY d ORDER BY d ASC",
            total = TOTAL_TOKENS_EXPR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raw: Vec<(String, i64, i64)> = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let max = raw.iter().map(|r| r.1).max().unwrap_or(0);
        let cells = raw
            .into_iter()
            .map(|(date, tokens, cost)| HeatmapCell {
                date,
                total_tokens: u(tokens),
                est_cost_microusd: u(cost),
                level: level(tokens, max),
            })
            .collect();
        Ok((cells, u(max), date_minus_days(&today, 30)))
    }
}

/// 0–4 intensity bucket of a day's tokens against the busiest day in range.
fn level(tokens: i64, max: i64) -> u8 {
    if tokens <= 0 || max <= 0 {
        return 0;
    }
    let ratio = tokens as f64 / max as f64;
    match ratio {
        r if r >= 0.75 => 4,
        r if r >= 0.5 => 3,
        r if r >= 0.25 => 2,
        _ => 1,
    }
}
