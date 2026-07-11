//! hermes (Nous Research) SQLite adapter. Reads the `sessions` summary table
//! of another tool's `state.db` strictly read-only and emits one cumulative
//! `UsageEvent` per session row.
//!
//! hermes has no per-row update timestamp, so incremental cursoring is
//! impossible; the table is small (hundreds of rows) and a full read per tick
//! is cheap. Every scan re-reads all usage-bearing rows and the keep-higher
//! upsert folds cumulative growth in place. `SELECT *` + a column-name map
//! tolerates schema drift across hermes versions v5..v20: only columns that
//! exist are read, each with a per-column default, and the sole required column
//! is `id`. The unreleased v20 `session_model_usage` table is ignored — it is
//! additive to the `sessions` counters we already read, so touching it would
//! double-count.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::model::{CostBasis, UsageEvent};
use crate::timeutil::{epoch_ms_to_rfc3339, epoch_to_rfc3339, parse_rfc3339_to_epoch};

use super::claude_code::project_from_cwd;
use super::{DbAdapter, DbFetch, SourceCursorState};

const SOURCE: &str = "hermes";

/// hermes epoch values above this are milliseconds, not seconds. `10^12` s is
/// year 33658; no real `started_at` in seconds reaches it, and a real ms value
/// (13 digits, ~1.7e12) sits above it.
const MS_EPOCH_THRESHOLD: i64 = 1_000_000_000_000;

pub struct HermesAdapter;

impl HermesAdapter {
    pub const SOURCE: &'static str = SOURCE;
}

impl DbAdapter for HermesAdapter {
    fn source(&self) -> &'static str {
        SOURCE
    }

    fn discover(&self) -> Option<PathBuf> {
        discover_from(
            std::env::var_os("RUNTAB_HERMES_DB").map(PathBuf::from),
            std::env::var_os("HERMES_HOME").map(PathBuf::from),
            crate::home_dir(),
        )
    }

    fn fetch(&self, db_path: &Path, _stored: Option<&SourceCursorState>) -> anyhow::Result<DbFetch> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening hermes db at {}", db_path.display()))?;
        conn.busy_timeout(Duration::from_millis(2000))
            .with_context(|| format!("setting busy_timeout on hermes db at {}", db_path.display()))?;
        read_sessions(&conn)
            .with_context(|| format!("reading sessions from hermes db at {}", db_path.display()))
    }
}

/// `RUNTAB_HERMES_DB` override (authoritative — set-but-missing yields `None`,
/// never a fall-through to a default), then `$HERMES_HOME/state.db`, then
/// `~/.hermes/state.db`. hermes is not XDG-based and the Windows
/// `%LOCALAPPDATA%` path is ignored (POSIX target, matching hermes' own
/// default). `None` means hermes isn't installed on this machine.
fn discover_from(
    override_path: Option<PathBuf>,
    hermes_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.exists().then_some(path);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(h) = hermes_home.filter(|p| !p.as_os_str().is_empty()) {
        candidates.push(h.join("state.db"));
    }
    if let Some(home) = &home {
        candidates.push(home.join(".hermes/state.db"));
    }
    candidates.into_iter().find(|p| p.exists())
}

fn read_sessions(conn: &Connection) -> anyhow::Result<DbFetch> {
    let mut stmt = conn.prepare("SELECT * FROM sessions")?;
    let col_index: HashMap<String, usize> = stmt
        .column_names()
        .iter()
        .enumerate()
        .map(|(i, name)| (name.to_string(), i))
        .collect();
    if !col_index.contains_key("id") {
        anyhow::bail!("hermes sessions table has no id column");
    }

    let mut events = Vec::new();
    let mut rows_skipped = 0u64;
    let mut row_count = 0i64;

    let mut query = stmt.query([])?;
    while let Some(row) = query.next()? {
        row_count += 1;
        match build_event(row, &col_index) {
            RowOutcome::Event(e) => events.push(*e),
            RowOutcome::Skip => rows_skipped += 1,
            RowOutcome::Ignore => {}
        }
    }

    Ok(DbFetch {
        events,
        rows_skipped,
        // hermes has no update-time column, so the whole table is re-read every
        // tick; the cursor carries no watermark.
        new_cursor: String::new(),
        row_count,
    })
}

enum RowOutcome {
    Event(Box<UsageEvent>),
    /// Usage-bearing row with an unusable timestamp (a garbage ts must never
    /// reach the server's ts validation): counted, not emitted.
    Skip,
    /// Zero-usage ghost row (no tokens): not an error, not counted, not emitted.
    Ignore,
}

fn build_event(row: &rusqlite::Row, col: &HashMap<String, usize>) -> RowOutcome {
    let input = token(row, col, "input_tokens");
    let output = token(row, col, "output_tokens");
    let cache_read = token(row, col, "cache_read_tokens");
    let cache_creation = token(row, col, "cache_write_tokens");
    let reasoning = token(row, col, "reasoning_tokens");

    if input + output + cache_read + cache_creation + reasoning == 0 {
        return RowOutcome::Ignore;
    }

    let ts = match normalize_ts(row, col, "started_at").or_else(|| normalize_ts(row, col, "ended_at"))
    {
        Some(ts) => ts,
        None => return RowOutcome::Skip,
    };

    // A non-TEXT or empty id would collide every such row on the dedup key
    // (source, "", ""), silently merging distinct sessions — skip instead.
    let session_id = match text(row, col, "id") {
        Some(id) if !id.is_empty() => id,
        _ => return RowOutcome::Skip,
    };
    let model = match text(row, col, "model") {
        Some(m) if !m.is_empty() => m,
        _ => "unknown".to_string(),
    };
    let project = match text(row, col, "cwd") {
        Some(c) if !c.is_empty() => project_from_cwd(&c),
        _ => text(row, col, "git_repo_root").unwrap_or_default(),
    };

    let (cost_usd, cost_basis) = cost_mapping(row, col);

    RowOutcome::Event(Box::new(UsageEvent {
        source: SOURCE.to_string(),
        message_id: session_id.clone(),
        request_id: String::new(),
        session_id,
        ts,
        model,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cache_1h_tokens: 0,
        cache_5m_tokens: 0,
        reasoning_tokens: reasoning,
        project,
        agent_version: String::new(),
        cost_usd,
        cost_basis,
    }))
}

/// hermes cost mapping (spec §6.5), evaluated top-down, first match wins.
fn cost_mapping(row: &rusqlite::Row, col: &HashMap<String, usize>) -> (Option<f64>, CostBasis) {
    let cost_status = text(row, col, "cost_status");
    let billing_mode = text(row, col, "billing_mode");
    let estimated = real(row, col, "estimated_cost_usd");
    let actual = real(row, col, "actual_cost_usd");

    // 1. A provider-reported real dollar figure is exactly what Logged means.
    if let Some(c) = actual {
        if c > 0.0 {
            return (Some(c), CostBasis::Logged);
        }
    }

    let status = cost_status.as_deref();
    let mode = billing_mode.as_deref();

    if status == Some("estimated") {
        match mode {
            // 2. Genuinely metered routes: the agent's own logged dollar amount.
            Some("official_models_api") | Some("official_docs_snapshot") => {
                if let Some(c) = estimated.filter(|c| *c > 0.0) {
                    return (Some(c), CostBasis::Logged);
                }
            }
            // 3. Subscription route must never look metered.
            Some("subscription_included") => return (None, CostBasis::Estimated),
            // 4. unknown/NULL/other: keep the figure, keep it in the sub bucket.
            _ => {
                if let Some(c) = estimated.filter(|c| *c > 0.0) {
                    return (Some(c), CostBasis::Estimated);
                }
            }
        }
    }

    // 5/6. included, or anything else (unknown/NULL status, NULL/0 estimate):
    // no logged figure — let pricing.apply fill an honest estimate if it can.
    (None, CostBasis::Estimated)
}

/// A token counter: Integer or Real storage → i64, negatives clamped to 0;
/// missing column or NULL → 0. Never fails the row (a corrupt count reads as 0
/// rather than skipping real spend on the other counters).
fn token(row: &rusqlite::Row, col: &HashMap<String, usize>, name: &str) -> i64 {
    let Some(&i) = col.get(name) else { return 0 };
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v.max(0),
        Ok(ValueRef::Real(v)) => (v as i64).max(0),
        _ => 0,
    }
}

/// A TEXT column as an owned string; missing column or non-text → `None`.
fn text(row: &rusqlite::Row, col: &HashMap<String, usize>, name: &str) -> Option<String> {
    let &i = col.get(name)?;
    match row.get_ref(i) {
        Ok(ValueRef::Text(b)) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// A REAL/INTEGER cost column as f64; missing column, NULL, or non-numeric →
/// `None`.
fn real(row: &rusqlite::Row, col: &HashMap<String, usize>, name: &str) -> Option<f64> {
    let &i = col.get(name)?;
    match row.get_ref(i) {
        Ok(ValueRef::Real(v)) => Some(v),
        Ok(ValueRef::Integer(v)) => Some(v as f64),
        _ => None,
    }
}

/// Normalize a hermes timestamp column to millisecond RFC 3339 UTC. Accepts:
/// REAL/INTEGER epoch seconds (values above `MS_EPOCH_THRESHOLD` are treated as
/// milliseconds), and TEXT ISO (`' '`→`'T'`, a trailing `Z` appended when the
/// string is zone-less). Missing column, NULL, or an unparseable string → `None`.
fn normalize_ts(row: &rusqlite::Row, col: &HashMap<String, usize>, name: &str) -> Option<String> {
    let &i = col.get(name)?;
    match row.get_ref(i).ok()? {
        ValueRef::Real(v) => Some(epoch_to_rfc3339_scaled(v as i64)),
        ValueRef::Integer(v) => Some(epoch_to_rfc3339_scaled(v)),
        ValueRef::Text(b) => normalize_text_ts(&String::from_utf8_lossy(b)),
        _ => None,
    }
}

fn epoch_to_rfc3339_scaled(v: i64) -> String {
    if v >= MS_EPOCH_THRESHOLD {
        epoch_ms_to_rfc3339(v)
    } else {
        epoch_to_rfc3339(v)
    }
}

fn normalize_text_ts(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // An explicit UTC offset would be silently misread as UTC by the 19-byte
    // parser (the offset never reaches it) — reject so the row is skipped
    // rather than misattributed by hours.
    if trimmed.get(19..).map_or(false, |tail| tail.contains('+') || tail.contains('-')) {
        return None;
    }
    // `parse_rfc3339_to_epoch` accepts both `T` and space separators; re-emit
    // as canonical `T…Z` so all sources sort uniformly.
    let secs = parse_rfc3339_to_epoch(trimmed)?;
    Some(epoch_to_rfc3339(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoned_text_timestamp_is_rejected_not_misread() {
        assert_eq!(normalize_text_ts("2026-07-11 09:00:00+02:00"), None);
        assert_eq!(normalize_text_ts("2026-07-11T09:00:00-07:00"), None);
        assert_eq!(
            normalize_text_ts("2026-07-11 09:00:00").as_deref(),
            Some("2026-07-11T09:00:00Z")
        );
        assert_eq!(
            normalize_text_ts("2026-07-11T09:00:00Z").as_deref(),
            Some("2026-07-11T09:00:00Z")
        );
    }

    #[test]
    fn discover_prefers_override_then_hermes_home_then_home() {
        let dir = std::env::temp_dir().join(format!("runtab_hermes_disc_{}", std::process::id()));
        let hermes_home = dir.join("hh");
        let home = dir.join("home");
        let _ = std::fs::create_dir_all(hermes_home.join(""));
        let _ = std::fs::create_dir_all(home.join(".hermes"));

        // Nothing present → None.
        assert_eq!(
            discover_from(None, Some(hermes_home.clone()), Some(home.clone())),
            None
        );

        // $HERMES_HOME/state.db present → chosen.
        let hh_db = hermes_home.join("state.db");
        std::fs::write(&hh_db, b"x").unwrap();
        assert_eq!(
            discover_from(None, Some(hermes_home.clone()), Some(home.clone())),
            Some(hh_db.clone())
        );

        // ~/.hermes/state.db present but $HERMES_HOME wins.
        let home_db = home.join(".hermes/state.db");
        std::fs::write(&home_db, b"x").unwrap();
        assert_eq!(
            discover_from(None, Some(hermes_home.clone()), Some(home.clone())),
            Some(hh_db)
        );

        // No $HERMES_HOME → ~/.hermes/state.db.
        assert_eq!(
            discover_from(None, None, Some(home.clone())),
            Some(home_db.clone())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_override_set_but_missing_never_falls_through() {
        let dir = std::env::temp_dir().join(format!("runtab_hermes_ovr_{}", std::process::id()));
        let home = dir.join("home");
        let _ = std::fs::create_dir_all(home.join(".hermes"));
        std::fs::write(home.join(".hermes/state.db"), b"x").unwrap();

        let missing = dir.join("nope/state.db");
        assert_eq!(
            discover_from(Some(missing), None, Some(home)),
            None,
            "a set-but-missing override must not fall through to the default"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
