//! `GET /api/savings`: rtk's own savings against the *real* token consumption
//! the rest of the dashboard headlines. Unlike `/api/tools` (which ducks the
//! global filters because `rtk_events` carry no `project_label`/`machine_id`),
//! this recovers scope through attribution: attributed rtk rows carry a
//! `session_id`, so joining `rtk_events.session_id` → `merged_events.session_id`
//! recovers the project/machine and lets the same `Filter` scope both sides.
//!
//! Unattributed rows (`match_kind = 'none'`) have no session and cannot be
//! scoped, so they are surfaced as a separate `saved_unattributed` figure only
//! when no project/machine/agent filter is set — never folded into the headline ratio
//! (the anti-vanity guard: grep-noise that never reached a model stays visible
//! and separate).

use rusqlite::params_from_iter;
use serde::Serialize;

use super::api_query::Filter;
use super::{schema::TOTAL_TOKENS_EXPR, Ledger};

#[derive(Debug, Serialize)]
pub struct SavingsWindow {
    /// Denominator: total consumed tokens over `merged_events`, same filter.
    pub consumed_tokens: u64,
    /// rtk savings tied to a real in-filter session (the headline numerator).
    pub saved_attributed: u64,
    /// `match_kind='none'` savings in the from/to window; `null` when a project,
    /// machine or agent filter is set (they can't be scoped to one).
    pub saved_unattributed: Option<u64>,
    /// Attributed command count in-filter.
    pub commands: u64,
    /// `saved_attributed / consumed_tokens`; `null` when `consumed_tokens == 0`.
    pub savings_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct SavingsDaily {
    pub date: String,
    pub consumed: u64,
    pub saved: u64,
}

#[derive(Debug, Serialize)]
pub struct SavingsReport {
    pub window: SavingsWindow,
    pub daily: Vec<SavingsDaily>,
    /// `false` when `rtk_events` is empty (rtk never imported); the UI hides
    /// the panel and the figures above are zeroed.
    pub rtk_present: bool,
}

fn u(v: i64) -> u64 {
    v.max(0) as u64
}

impl Ledger {
    pub fn savings(&self, f: &Filter) -> rusqlite::Result<SavingsReport> {
        let rtk_present: bool =
            self.conn
                .query_row("SELECT EXISTS(SELECT 1 FROM rtk_events)", [], |r| r.get::<_, i64>(0))?
                != 0;
        if !rtk_present {
            return Ok(SavingsReport {
                window: SavingsWindow {
                    consumed_tokens: 0,
                    saved_attributed: 0,
                    saved_unattributed: None,
                    commands: 0,
                    savings_ratio: None,
                },
                daily: Vec::new(),
                rtk_present: false,
            });
        }

        let (where_sql, p) = f.clause();

        // Denominator — identical path to `/api/summary.total_tokens`.
        let consumed_sql = format!("SELECT COALESCE(SUM({TOTAL_TOKENS_EXPR}),0) FROM merged_events{where_sql}");
        let consumed: i64 = self
            .conn
            .query_row(&consumed_sql, params_from_iter(p.iter()), |r| r.get(0))?;

        // Attributed savings + command count: rtk rows whose recovered session
        // is in the filtered `merged_events`. `session_id IN (…)` membership is
        // sufficient — a session maps to one project/machine in practice, so no
        // per-row disambiguation is needed for the totals (spec §7). `ts`
        // window bounds are applied to `rtk_events.ts` directly.
        let (rtk_from, rtk_to) = rtk_window_clause(f);
        let mut rtk_p: Vec<String> = Vec::new();
        if let Some(v) = &f.from {
            rtk_p.push(v.clone());
        }
        if let Some(v) = &f.to {
            rtk_p.push(v.clone());
        }
        rtk_p.extend(p.iter().cloned());
        let attributed_sql = format!(
            "SELECT COALESCE(SUM(r.saved_tokens),0), COUNT(*)
             FROM rtk_events r
             WHERE r.match_kind != 'none'{rtk_from}{rtk_to}
               AND r.session_id IN (SELECT session_id FROM merged_events{where_sql})"
        );
        let (saved_attributed, commands): (i64, i64) = self
            .conn
            .query_row(&attributed_sql, params_from_iter(rtk_p.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;

        // Unattributed savings: only surfaced with no project/machine/agent
        // filter set (they can't be scoped to one). Honours from/to.
        let scoped = f.project.is_some() || f.machine.is_some() || f.agent.is_some();
        let saved_unattributed = if scoped {
            None
        } else {
            let mut up: Vec<String> = Vec::new();
            if let Some(v) = &f.from {
                up.push(v.clone());
            }
            if let Some(v) = &f.to {
                up.push(v.clone());
            }
            let sql = format!(
                "SELECT COALESCE(SUM(r.saved_tokens),0) FROM rtk_events r
                 WHERE r.match_kind = 'none'{rtk_from}{rtk_to}"
            );
            let v: i64 = self.conn.query_row(&sql, params_from_iter(up.iter()), |r| r.get(0))?;
            Some(u(v))
        };

        let savings_ratio = if consumed > 0 {
            Some(saved_attributed as f64 / consumed as f64)
        } else {
            None
        };

        let daily = self.savings_daily(f, &where_sql, &p, &rtk_from, &rtk_to)?;

        Ok(SavingsReport {
            window: SavingsWindow {
                consumed_tokens: u(consumed),
                saved_attributed: u(saved_attributed),
                saved_unattributed,
                commands: u(commands),
                savings_ratio,
            },
            daily,
            rtk_present: true,
        })
    }

    /// Per-day consumed (`merged_events`) and attributed saved (`rtk_events`),
    /// merged on date. `saved` respects the same filter as `saved_attributed`.
    fn savings_daily(
        &self,
        f: &Filter,
        where_sql: &str,
        p: &[String],
        rtk_from: &str,
        rtk_to: &str,
    ) -> rusqlite::Result<Vec<SavingsDaily>> {
        let consumed_sql = format!(
            "SELECT substr(ts,1,10) d, COALESCE(SUM({TOTAL_TOKENS_EXPR}),0)
             FROM merged_events{where_sql} GROUP BY d ORDER BY d ASC"
        );
        let mut stmt = self.conn.prepare(&consumed_sql)?;
        let consumed_rows: Vec<(String, i64)> = stmt
            .query_map(params_from_iter(p.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut rtk_p: Vec<String> = Vec::new();
        if let Some(v) = &f.from {
            rtk_p.push(v.clone());
        }
        if let Some(v) = &f.to {
            rtk_p.push(v.clone());
        }
        rtk_p.extend(p.iter().cloned());
        let saved_sql = format!(
            "SELECT substr(r.ts,1,10) d, COALESCE(SUM(r.saved_tokens),0)
             FROM rtk_events r
             WHERE r.match_kind != 'none'{rtk_from}{rtk_to}
               AND r.session_id IN (SELECT session_id FROM merged_events{where_sql})
             GROUP BY d"
        );
        let mut stmt = self.conn.prepare(&saved_sql)?;
        let saved_rows: Vec<(String, i64)> = stmt
            .query_map(params_from_iter(rtk_p.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut out: Vec<SavingsDaily> = consumed_rows
            .into_iter()
            .map(|(date, consumed)| SavingsDaily { date, consumed: u(consumed), saved: 0 })
            .collect();
        for (date, saved) in saved_rows {
            match out.iter_mut().find(|d| d.date == date) {
                Some(d) => d.saved = u(saved),
                // A saved-only day (rtk ran but no consumption landed under the
                // filter): keep it so the two series stay aligned on all dates.
                None => out.push(SavingsDaily { date, consumed: 0, saved: u(saved) }),
            }
        }
        out.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(out)
    }
}

/// The from/to fragments applied to `rtk_events.ts` (the command's run time).
/// Bound values are pushed by the caller in this order (from, then to).
fn rtk_window_clause(f: &Filter) -> (String, String) {
    let from = if f.from.is_some() {
        " AND substr(r.ts,1,10) >= ?".to_string()
    } else {
        String::new()
    };
    let to = if f.to.is_some() {
        " AND substr(r.ts,1,10) <= ?".to_string()
    } else {
        String::new()
    };
    (from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RtkCommandRow;

    fn filter() -> Filter {
        Filter::default()
    }

    /// Insert one consumption row straight into `usage_events`, controlling the
    /// `project_label`/`machine_id`/`session_id`/`ts` the merged view exposes.
    fn consume(l: &Ledger, session: &str, project: &str, machine: &str, ts: &str, total: i64) {
        consume_src(l, "claude_code", session, project, machine, ts, total);
    }

    /// `consume` with an explicit `source` (agent id in local underscore form),
    /// so the agent filter can be exercised across sources.
    fn consume_src(l: &Ledger, source: &str, session: &str, project: &str, machine: &str, ts: &str, total: i64) {
        l.conn
            .execute(
                "INSERT INTO usage_events
                    (source, message_id, request_id, session_id, ts, model,
                     input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                     cache_1h_tokens, cache_5m_tokens, reasoning_tokens,
                     project, agent_version, cost_usd, cost_basis,
                     project_label, machine_id, machine_name)
                 VALUES (?7, ?1, ?1, ?2, ?3, 'm', ?4, 0, 0, 0, 0, 0, 0,
                         ?5, 'v', NULL, 'estimated', ?5, ?6, ?6)",
                rusqlite::params![
                    format!("{session}-{ts}-{total}"),
                    session,
                    ts,
                    total,
                    project,
                    machine,
                    source
                ],
            )
            .unwrap();
    }

    /// Import one rtk row, then attribute it to a session (`match_kind='text'`).
    fn saved_attributed(l: &Ledger, rtk_id: i64, session: &str, ts: &str, saved: i64) {
        insert_rtk(l, rtk_id, ts, saved);
        l.update_rtk_attribution(rtk_row_internal_id(l, rtk_id), "claude_code", session, 1, "text")
            .unwrap();
    }

    /// Import one rtk row that stays `match_kind='none'`.
    fn saved_unattributed(l: &Ledger, rtk_id: i64, ts: &str, saved: i64) {
        insert_rtk(l, rtk_id, ts, saved);
    }

    fn insert_rtk(l: &Ledger, rtk_id: i64, ts: &str, saved: i64) {
        l.insert_rtk_event(&RtkCommandRow {
            rtk_row_id: rtk_id,
            ts: ts.to_string(),
            project_path: "/home/u/p".to_string(),
            head_hash: "h".to_string(),
            cmd_hash: "c".to_string(),
            raw_tokens: saved,
            filtered_tokens: 0,
            saved_tokens: saved,
            exec_time_ms: 0,
        })
        .unwrap();
    }

    fn rtk_row_internal_id(l: &Ledger, rtk_row_id: i64) -> i64 {
        l.conn
            .query_row(
                "SELECT id FROM rtk_events WHERE rtk_row_id = ?1",
                rusqlite::params![rtk_row_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn rtk_present_false_on_empty_rtk_events() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s1", "proj", "mach", "2026-07-01T00:00:00Z", 1000);

        let r = l.savings(&filter()).unwrap();
        assert!(!r.rtk_present);
        assert_eq!(r.window.consumed_tokens, 0);
        assert_eq!(r.window.saved_attributed, 0);
        assert_eq!(r.window.saved_unattributed, None);
        assert_eq!(r.window.savings_ratio, None);
        assert!(r.daily.is_empty());
    }

    #[test]
    fn savings_ratio_null_on_zero_consumption() {
        let l = Ledger::open_in_memory().unwrap();
        // rtk present but no consumption at all.
        saved_unattributed(&l, 1, "2026-07-01T00:00:00Z", 500);

        let r = l.savings(&filter()).unwrap();
        assert!(r.rtk_present);
        assert_eq!(r.window.consumed_tokens, 0);
        assert_eq!(r.window.savings_ratio, None);
    }

    #[test]
    fn attributed_vs_unattributed_split() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s1", "proj", "mach", "2026-07-01T00:00:00Z", 10_000);
        saved_attributed(&l, 1, "s1", "2026-07-01T00:00:00Z", 800);
        saved_unattributed(&l, 2, "2026-07-01T00:00:00Z", 300);

        let r = l.savings(&filter()).unwrap();
        assert_eq!(r.window.consumed_tokens, 10_000);
        assert_eq!(r.window.saved_attributed, 800);
        assert_eq!(r.window.commands, 1);
        assert_eq!(r.window.saved_unattributed, Some(300));
        // Ratio is attributed-only: 800 / 10_000.
        assert_eq!(r.window.savings_ratio, Some(0.08));
    }

    #[test]
    fn project_filter_scopes_attributed_and_nulls_unattributed() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s_a", "alpha", "mach", "2026-07-01T00:00:00Z", 10_000);
        consume(&l, "s_b", "beta", "mach", "2026-07-01T00:00:00Z", 20_000);
        saved_attributed(&l, 1, "s_a", "2026-07-01T00:00:00Z", 800);
        saved_attributed(&l, 2, "s_b", "2026-07-01T00:00:00Z", 999);
        saved_unattributed(&l, 3, "2026-07-01T00:00:00Z", 300);

        let f = Filter {
            project: Some("alpha".to_string()),
            ..Filter::default()
        };
        let r = l.savings(&f).unwrap();
        // Only alpha's session survives the join.
        assert_eq!(r.window.consumed_tokens, 10_000);
        assert_eq!(r.window.saved_attributed, 800);
        assert_eq!(r.window.commands, 1);
        // Unattributed can't be scoped to a project ⇒ dropped/null.
        assert_eq!(r.window.saved_unattributed, None);
    }

    #[test]
    fn machine_filter_scopes_attributed_and_nulls_unattributed() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s_a", "proj", "m1", "2026-07-01T00:00:00Z", 10_000);
        consume(&l, "s_b", "proj", "m2", "2026-07-01T00:00:00Z", 20_000);
        saved_attributed(&l, 1, "s_a", "2026-07-01T00:00:00Z", 700);
        saved_attributed(&l, 2, "s_b", "2026-07-01T00:00:00Z", 900);
        saved_unattributed(&l, 3, "2026-07-01T00:00:00Z", 300);

        let f = Filter {
            machine: Some("m2".to_string()),
            ..Filter::default()
        };
        let r = l.savings(&f).unwrap();
        assert_eq!(r.window.consumed_tokens, 20_000);
        assert_eq!(r.window.saved_attributed, 900);
        assert_eq!(r.window.saved_unattributed, None);
    }

    #[test]
    fn agent_filter_scopes_attributed_and_nulls_unattributed() {
        let l = Ledger::open_in_memory().unwrap();
        // Two sources; the attributed rtk rows join back through session_id, so
        // scoping the merged side by `source` scopes the attributed savings too.
        consume_src(&l, "claude_code", "s_cc", "proj", "mach", "2026-07-01T00:00:00Z", 10_000);
        consume_src(&l, "codex", "s_cx", "proj", "mach", "2026-07-01T00:00:00Z", 20_000);
        saved_attributed(&l, 1, "s_cc", "2026-07-01T00:00:00Z", 800);
        saved_attributed(&l, 2, "s_cx", "2026-07-01T00:00:00Z", 999);
        saved_unattributed(&l, 3, "2026-07-01T00:00:00Z", 300);

        let f = Filter {
            agent: Some("codex".to_string()),
            ..Filter::default()
        };
        let r = l.savings(&f).unwrap();
        // Only codex's session survives the join.
        assert_eq!(r.window.consumed_tokens, 20_000);
        assert_eq!(r.window.saved_attributed, 999);
        assert_eq!(r.window.commands, 1);
        // Unattributed rows carry no source ⇒ can't be scoped to an agent ⇒ null.
        assert_eq!(r.window.saved_unattributed, None);
    }

    #[test]
    fn from_to_window_scopes_both_sides() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s1", "proj", "mach", "2026-07-01T00:00:00Z", 5_000);
        consume(&l, "s1", "proj", "mach", "2026-07-05T00:00:00Z", 7_000);
        saved_attributed(&l, 1, "s1", "2026-07-01T00:00:00Z", 400);
        saved_attributed(&l, 2, "s1", "2026-07-05T00:00:00Z", 600);
        saved_unattributed(&l, 3, "2026-07-01T00:00:00Z", 100);
        saved_unattributed(&l, 4, "2026-07-05T00:00:00Z", 200);

        let f = Filter {
            from: Some("2026-07-04".to_string()),
            to: Some("2026-07-06".to_string()),
            ..Filter::default()
        };
        let r = l.savings(&f).unwrap();
        assert_eq!(r.window.consumed_tokens, 7_000);
        assert_eq!(r.window.saved_attributed, 600);
        // No project/machine filter ⇒ unattributed surfaced, but only in-window.
        assert_eq!(r.window.saved_unattributed, Some(200));
    }

    #[test]
    fn daily_aligns_consumed_and_saved_on_same_dates() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s1", "proj", "mach", "2026-07-01T00:00:00Z", 1_000);
        consume(&l, "s1", "proj", "mach", "2026-07-02T00:00:00Z", 2_000);
        saved_attributed(&l, 1, "s1", "2026-07-01T00:00:00Z", 40);
        saved_attributed(&l, 2, "s1", "2026-07-02T00:00:00Z", 80);
        // An unattributed row never lands in `daily.saved`.
        saved_unattributed(&l, 3, "2026-07-02T00:00:00Z", 500);

        let r = l.savings(&filter()).unwrap();
        assert_eq!(r.daily.len(), 2);
        assert_eq!(r.daily[0].date, "2026-07-01");
        assert_eq!(r.daily[0].consumed, 1_000);
        assert_eq!(r.daily[0].saved, 40);
        assert_eq!(r.daily[1].date, "2026-07-02");
        assert_eq!(r.daily[1].consumed, 2_000);
        assert_eq!(r.daily[1].saved, 80);
    }

    #[test]
    fn attributed_total_matches_summary_denominator_path() {
        let l = Ledger::open_in_memory().unwrap();
        consume(&l, "s1", "proj", "mach", "2026-07-01T00:00:00Z", 3_000);
        consume(&l, "s2", "proj", "mach", "2026-07-02T00:00:00Z", 4_000);
        saved_attributed(&l, 1, "s1", "2026-07-01T00:00:00Z", 100);

        let savings = l.savings(&filter()).unwrap();
        let summary = l.api_summary(&filter()).unwrap();
        // The denominator is the exact same figure `/api/summary` headlines.
        assert_eq!(savings.window.consumed_tokens, summary.total_tokens);
    }
}
