//! Dogfood acceptance gate (spec §10). Every test here is `#[ignore]`: they read
//! this machine's *real* opencode / hermes / codex stores, so they are opt-in and
//! never run in CI. Run them with:
//!
//! ```text
//! cargo test -p runtab --test dogfood_tests -- --ignored --nocapture
//! ```
//!
//! Safety contract (mirrors the spec's binding constraints):
//!   * The real opencode/hermes DBs are opened strictly READ-ONLY, and only
//!     through the adapters (`scan_db_source_at`). These tests never issue a
//!     write against them.
//!   * The user's real runtab ledger is NEVER touched — every scan targets a
//!     throwaway temp ledger created per test. No bare `runtab scan` is ever run.
//!   * Nothing is written to `~/.local/share/opencode`, `~/.hermes`, or `~/.codex`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

use runtab::adapters::{Adapter, CodexAdapter, DbAdapter, HermesAdapter, OpencodeAdapter};
use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::{scan_db_source_at, scan_file, ScanSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh throwaway ledger path — never the developer's real ledger.
fn temp_ledger_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "runtab_dogfood_{prefix}_{}_{nanos}_{unique}.db",
        std::process::id()
    ))
}

/// Scan a DB-backed source at its real discovered path into a temp ledger.
fn scan_db_into(ledger: &Ledger, adapter: &dyn DbAdapter, db_path: &Path) -> ScanSummary {
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    scan_db_source_at(ledger, adapter, &pricing, db_path, &mut summary);
    summary
}

/// After a scan, wipe the incremental bookkeeping in the TEMP ledger only
/// (`source_cursors` for DB adapters, `scanned_files` for file adapters), forcing
/// the next scan to re-read every source from zero. A correct set of deterministic
/// identities must dedup a from-zero rescan to zero new inserts.
fn reset_incremental_state(ledger_path: &Path) {
    let conn = Connection::open(ledger_path).unwrap();
    conn.execute("DELETE FROM source_cursors", []).unwrap();
    conn.execute("DELETE FROM scanned_files", []).unwrap();
}

/// Per-source token/cost aggregates read straight off the temp ledger.
struct Sums {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    reasoning: i64,
    cost: Option<f64>,
    events: i64,
}

fn source_sums(ledger_path: &Path, source: &str) -> Sums {
    let conn = Connection::open(ledger_path).unwrap();
    conn.query_row(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                COALESCE(SUM(reasoning_tokens),0), SUM(cost_usd), COUNT(*)
         FROM usage_events WHERE source = ?1",
        [source],
        |r| {
            Ok(Sums {
                input: r.get(0)?,
                output: r.get(1)?,
                cache_read: r.get(2)?,
                cache_creation: r.get(3)?,
                reasoning: r.get(4)?,
                cost: r.get(5)?,
                events: r.get(6)?,
            })
        },
    )
    .unwrap()
}

/// Aggregates for a single opencode session id.
fn opencode_session_sums(ledger_path: &Path, session_id: &str) -> (i64, i64, i64, i64, i64, Option<f64>, i64, i64) {
    let conn = Connection::open(ledger_path).unwrap();
    conn.query_row(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                COALESCE(SUM(reasoning_tokens),0), SUM(cost_usd),
                COUNT(*), COALESCE(SUM(cost_basis = 'estimated'),0)
         FROM usage_events WHERE source = 'opencode' AND session_id = ?1",
        [session_id],
        |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?,
            ))
        },
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// opencode — real DB, exact known totals (spec §10).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "reads the machine's real ~/.local/share/opencode/opencode.db"]
fn opencode_real_session_sums_match() {
    let Some(db_path) = OpencodeAdapter.discover() else {
        eprintln!("opencode DB not present — skipping opencode dogfood");
        return;
    };
    let ledger_path = temp_ledger_path("opencode");
    let ledger = Ledger::open(&ledger_path).unwrap();
    let s = scan_db_into(&ledger, &OpencodeAdapter, &db_path);
    println!(
        "[opencode] db={} files_scanned={} events_inserted={} rows_skipped={} db_errors={}",
        db_path.display(),
        s.files_scanned,
        s.events_inserted,
        s.lines_skipped,
        s.db_errors
    );
    assert_eq!(s.db_errors, 0, "opencode scan reported db errors");

    let session_id = "ses_38c6fd43affeIC7KJLgUy77H54";
    let (input, output, cache_read, cache_creation, reasoning, cost, events, estimated) =
        opencode_session_sums(&ledger_path, session_id);
    println!(
        "[opencode] session {session_id}: input={input} output={output} cache_read={cache_read} \
         cache_creation={cache_creation} reasoning={reasoning} cost={:?} events={events} \
         estimated_rows={estimated}",
        cost
    );

    assert_eq!(input, 46_944, "opencode input sum");
    assert_eq!(output, 20_944, "opencode output sum");
    assert_eq!(cache_read, 5_020_617, "opencode cache_read sum");
    assert_eq!(cache_creation, 96_263, "opencode cache_creation sum");

    let cost = cost.expect("opencode session must carry a summed cost");
    assert!(
        (cost - 0.4158).abs() <= 0.005,
        "opencode SUM(cost_usd)={cost} outside 0.4158 ± 0.005"
    );
    // opencode cost is a synthetic models.dev figure → every row is Estimated.
    assert_eq!(
        estimated, events,
        "every opencode row must be cost_basis='estimated' ({estimated}/{events})"
    );
}

// ---------------------------------------------------------------------------
// hermes — real DB, floor totals (the live DB is mutable → assert >=, print actuals).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "reads the machine's real ~/.hermes/state.db"]
fn hermes_real_source_sums_meet_floors() {
    let Some(db_path) = HermesAdapter.discover() else {
        eprintln!("hermes DB not present — skipping hermes dogfood");
        return;
    };
    let ledger_path = temp_ledger_path("hermes");
    let ledger = Ledger::open(&ledger_path).unwrap();
    let s = scan_db_into(&ledger, &HermesAdapter, &db_path);
    println!(
        "[hermes] db={} files_scanned={} events_inserted={} rows_skipped={} db_errors={}",
        db_path.display(),
        s.files_scanned,
        s.events_inserted,
        s.lines_skipped,
        s.db_errors
    );
    assert_eq!(s.db_errors, 0, "hermes scan reported db errors");

    let sums = source_sums(&ledger_path, "hermes");
    println!(
        "[hermes] source sums: input={} output={} cache_read={} cache_creation={} reasoning={} \
         cost={:?} events={}",
        sums.input, sums.output, sums.cache_read, sums.cache_creation, sums.reasoning, sums.cost, sums.events
    );

    // The live DB grows over time → floors, not equalities.
    assert!(sums.input >= 1_685_887, "hermes input {} below floor 1_685_887", sums.input);
    assert!(sums.output >= 287_982, "hermes output {} below floor 287_982", sums.output);
    assert!(
        sums.cache_read >= 21_996_681,
        "hermes cache_read {} below floor 21_996_681",
        sums.cache_read
    );
}

// ---------------------------------------------------------------------------
// codex — real ~/.codex tree if present; must skip gracefully when absent.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "reads the machine's real ~/.codex tree if present"]
fn codex_real_tree_scans_or_skips_gracefully() {
    let adapter = CodexAdapter;
    let files = adapter.discover();
    println!("[codex] discovered {} rollout file(s)", files.len());

    if files.is_empty() {
        // ~/.codex absent (or empty): discovery must not panic and must yield an
        // empty set. Nothing to scan — the source is silently off. This is the
        // expected state on a machine without Codex installed.
        println!("[codex] no ~/.codex rollouts present — graceful skip verified");
        return;
    }

    let ledger_path = temp_ledger_path("codex");
    let ledger = Ledger::open(&ledger_path).unwrap();
    let pricing = Pricing::load().unwrap();
    let mut summary = ScanSummary::default();
    for path in &files {
        scan_file(&ledger, &adapter, &pricing, path, &mut summary);
    }
    println!(
        "[codex] files_scanned={} events_inserted={} lines_skipped={} db_errors={}",
        summary.files_scanned, summary.events_inserted, summary.lines_skipped, summary.db_errors
    );
    assert_eq!(summary.db_errors, 0, "codex scan reported db errors");

    // A second full sweep with the file offsets intact re-reads nothing new;
    // deterministic identities dedup any re-parse to zero inserts.
    let mut rescan = ScanSummary::default();
    for path in &files {
        scan_file(&ledger, &adapter, &pricing, path, &mut rescan);
    }
    println!("[codex] rescan events_inserted={}", rescan.events_inserted);
    assert_eq!(rescan.events_inserted, 0, "codex rescan inserted new rows");
    assert_eq!(rescan.db_errors, 0, "codex rescan reported db errors");
}

// ---------------------------------------------------------------------------
// Idempotency on real data, per present source: scan → wipe cursors/scanned_files
// in the TEMP ledger → rescan from zero → 0 new inserts.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "reads the machine's real opencode/hermes/codex stores"]
fn rescan_from_zero_inserts_nothing_for_all_present_sources() {
    let ledger_path = temp_ledger_path("idempotency");
    let ledger = Ledger::open(&ledger_path).unwrap();
    let pricing = Pricing::load().unwrap();

    let opencode_db = OpencodeAdapter.discover();
    let hermes_db = HermesAdapter.discover();
    let codex_files = CodexAdapter.discover();

    println!(
        "[idempotency] present sources: opencode={} hermes={} codex_files={}",
        opencode_db.is_some(),
        hermes_db.is_some(),
        codex_files.len()
    );

    // --- initial full scan of every present source into the temp ledger ---
    let mut first = ScanSummary::default();
    if let Some(ref p) = opencode_db {
        scan_db_source_at(&ledger, &OpencodeAdapter, &pricing, p, &mut first);
    }
    if let Some(ref p) = hermes_db {
        scan_db_source_at(&ledger, &HermesAdapter, &pricing, p, &mut first);
    }
    let codex_adapter = CodexAdapter;
    for path in &codex_files {
        scan_file(&ledger, &codex_adapter, &pricing, path, &mut first);
    }
    println!(
        "[idempotency] first scan: events_inserted={} db_errors={}",
        first.events_inserted, first.db_errors
    );
    assert_eq!(first.db_errors, 0, "first scan reported db errors");
    let rows_after_first = source_sums(&ledger_path, "opencode").events
        + source_sums(&ledger_path, "hermes").events
        + source_sums(&ledger_path, "codex").events;

    // --- wipe incremental state in the TEMP ledger, forcing a from-zero rescan ---
    reset_incremental_state(&ledger_path);

    let mut second = ScanSummary::default();
    if let Some(ref p) = opencode_db {
        scan_db_source_at(&ledger, &OpencodeAdapter, &pricing, p, &mut second);
    }
    if let Some(ref p) = hermes_db {
        scan_db_source_at(&ledger, &HermesAdapter, &pricing, p, &mut second);
    }
    for path in &codex_files {
        scan_file(&ledger, &codex_adapter, &pricing, path, &mut second);
    }
    println!(
        "[idempotency] rescan-from-zero: events_inserted={} duplicates_dropped={} db_errors={}",
        second.events_inserted, second.duplicates_dropped, second.db_errors
    );

    assert_eq!(second.db_errors, 0, "rescan reported db errors");
    assert_eq!(
        second.events_inserted, 0,
        "rescan-from-zero inserted {} new rows — deterministic identities must dedup to zero",
        second.events_inserted
    );

    // Row counts are unchanged by the rescan (deleted-upstream rows also persist).
    let rows_after_second = source_sums(&ledger_path, "opencode").events
        + source_sums(&ledger_path, "hermes").events
        + source_sums(&ledger_path, "codex").events;
    assert_eq!(
        rows_after_first, rows_after_second,
        "row count changed across an idempotent rescan"
    );
}
