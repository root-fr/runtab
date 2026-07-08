use rusqlite::params_from_iter;
use serde::Serialize;

use super::{schema::TOTAL_TOKENS_EXPR, Ledger};
use crate::billing::{self, Mode};
use crate::timeutil::today_utc;

/// Global dashboard filters shared by every `/api/*` endpoint.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl Filter {
    /// A ` WHERE …` fragment plus its bound values, in textual (positional)
    /// order. All bound values are strings, so callers appending extra `?`
    /// placeholders must keep the same ordering.
    pub(super) fn clause(&self) -> (String, Vec<String>) {
        let mut sql = String::from(" WHERE 1=1");
        let mut p = Vec::new();
        if let Some(v) = &self.project {
            sql.push_str(" AND project_label = ?");
            p.push(v.clone());
        }
        if let Some(v) = &self.machine {
            sql.push_str(" AND machine_id = ?");
            p.push(v.clone());
        }
        if let Some(v) = &self.from {
            sql.push_str(" AND substr(ts,1,10) >= ?");
            p.push(v.clone());
        }
        if let Some(v) = &self.to {
            sql.push_str(" AND substr(ts,1,10) <= ?");
            p.push(v.clone());
        }
        (sql, p)
    }
}

#[derive(Debug, Serialize)]
pub struct ModeBreak {
    pub billing_mode: String,
    pub est_cost_microusd: u64,
    pub cost_label: String,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    pub cost_label: String,
    pub billing_mode: String,
    pub plan_window_applicable: bool,
    pub today_tokens: u64,
    pub today_est_cost_microusd: u64,
    pub cache_read_ratio: f64,
    pub project_count: u64,
    pub machine_count: u64,
    pub since: Option<String>,
    pub modes: Vec<ModeBreak>,
}

#[derive(Debug, Serialize)]
pub struct ModelRow {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    pub unpriced_events: u64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
pub struct ProjectRow {
    pub project_label: String,
    pub total_tokens: u64,
    pub est_cost_microusd: u64,
    pub session_count: u64,
    pub share: f64,
}

/// One row of the per-model aggregate: model, then the token/cost/unpriced sums.
type ModelAgg = (String, i64, i64, i64, i64, i64, i64, i64, i64);

fn u(v: i64) -> u64 {
    v.max(0) as u64
}

impl Ledger {
    pub fn api_summary(&self, f: &Filter) -> rusqlite::Result<Summary> {
        let (where_sql, mut p) = f.clause();
        let sql = format!(
            "SELECT COALESCE(SUM({total}),0), COALESCE(SUM(cache_read_tokens),0),
                    COALESCE(SUM(CASE WHEN cost_basis='estimated' THEN est_cost_microusd END),0),
                    COALESCE(SUM(CASE WHEN cost_basis IN ('logged','billed') THEN est_cost_microusd END),0),
                    COALESCE(SUM(CASE WHEN cost_basis='estimated' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN cost_basis IN ('logged','billed') THEN 1 ELSE 0 END),0),
                    COUNT(DISTINCT project_label), COUNT(DISTINCT machine_id),
                    MIN(substr(ts,1,10))
             FROM merged_events{where_sql}",
            total = TOTAL_TOKENS_EXPR
        );
        let row = self.conn.query_row(&sql, params_from_iter(p.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let (total, cache_read, sub_cost, api_cost, sub_ev, api_ev, projects, machines, since) = row;

        let today = today_utc();
        p.push(today);
        let tq = format!(
            "SELECT COALESCE(SUM({total}),0),
                    COALESCE(SUM(est_cost_microusd),0)
             FROM merged_events{where_sql} AND substr(ts,1,10) = ?",
            total = TOTAL_TOKENS_EXPR
        );
        let (today_tokens, today_cost): (i64, i64) =
            self.conn
                .query_row(&tq, params_from_iter(p.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;

        let mode = billing::resolve(self.override_mode()?, sub_ev, api_ev);
        let modes = match mode {
            Mode::Mixed => vec![
                mode_break(Mode::Subscription, sub_cost),
                mode_break(Mode::Api, api_cost),
            ],
            m => vec![mode_break(m, sub_cost + api_cost)],
        };
        let ratio = if total > 0 {
            cache_read as f64 / total as f64
        } else {
            0.0
        };
        Ok(Summary {
            total_tokens: u(total),
            est_cost_microusd: u(sub_cost + api_cost),
            cost_label: mode.label().to_string(),
            billing_mode: mode.as_str().to_string(),
            plan_window_applicable: mode.plan_applicable(),
            today_tokens: u(today_tokens),
            today_est_cost_microusd: u(today_cost),
            cache_read_ratio: ratio,
            project_count: u(projects),
            machine_count: u(machines),
            since,
            modes,
        })
    }

    pub fn api_models(&self, f: &Filter) -> rusqlite::Result<Vec<ModelRow>> {
        let (where_sql, p) = f.clause();
        let sql = format!(
            "SELECT model,
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM({total}),0),
                    COALESCE(SUM(est_cost_microusd),0), COALESCE(SUM(unpriced),0)
             FROM merged_events{where_sql} GROUP BY model ORDER BY 7 DESC",
            total = TOTAL_TOKENS_EXPR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<ModelAgg> = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                    r.get(7)?, r.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let grand: i64 = rows.iter().map(|r| r.6).sum();
        Ok(rows
            .into_iter()
            .map(|r| ModelRow {
                model: r.0,
                input_tokens: u(r.1),
                output_tokens: u(r.2),
                cache_read_tokens: u(r.3),
                cache_creation_tokens: u(r.4),
                reasoning_tokens: u(r.5),
                total_tokens: u(r.6),
                est_cost_microusd: u(r.7),
                unpriced_events: u(r.8),
                share: share(r.6, grand),
            })
            .collect())
    }

    pub fn api_projects(&self, f: &Filter) -> rusqlite::Result<Vec<ProjectRow>> {
        let (where_sql, p) = f.clause();
        let sql = format!(
            "SELECT project_label, COALESCE(SUM({total}),0),
                    COALESCE(SUM(est_cost_microusd),0), COUNT(DISTINCT session_id)
             FROM merged_events{where_sql} GROUP BY project_label ORDER BY 2 DESC",
            total = TOTAL_TOKENS_EXPR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<(String, i64, i64, i64)> = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let grand: i64 = rows.iter().map(|r| r.1).sum();
        Ok(rows
            .into_iter()
            .map(|r| ProjectRow {
                project_label: r.0,
                total_tokens: u(r.1),
                est_cost_microusd: u(r.2),
                session_count: u(r.3),
                share: share(r.1, grand),
            })
            .collect())
    }
}

fn mode_break(mode: Mode, cost: i64) -> ModeBreak {
    ModeBreak {
        billing_mode: mode.as_str().to_string(),
        est_cost_microusd: u(cost),
        cost_label: mode.label().to_string(),
    }
}

fn share(part: i64, whole: i64) -> f64 {
    if whole > 0 {
        part as f64 / whole as f64
    } else {
        0.0
    }
}
