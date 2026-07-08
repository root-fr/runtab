use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use runtab::ledger::Ledger;
use runtab::pricing::Pricing;
use runtab::sync;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "runtab_run_once_{label}_{}_{nanos}_{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A logged-out ledger (default sync state: disabled, no account, no review)
/// hits cron's tick with nothing to do — it must scan, log a `skip:` line,
/// and never mark sync degraded (there is nothing to degrade from).
#[test]
fn run_once_scans_only_when_logged_out() {
    let data_dir = temp_dir("data");
    let home_dir = temp_dir("home"); // isolates ClaudeCodeAdapter discovery from the real machine
    std::env::set_var("XDG_DATA_HOME", &data_dir);
    std::env::set_var("HOME", &home_dir);
    std::env::set_var("XDG_CONFIG_HOME", home_dir.join(".config"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");

    let db_path = data_dir.join("runtab_test.db");
    let ledger = Ledger::open(&db_path).unwrap();
    let pricing = Pricing::load().unwrap();

    sync::dispatch(ledger, &pricing, &db_path, sync::Cmd::Run { verbose: false }).unwrap();

    let ledger = Ledger::open(&db_path).unwrap();
    let s = ledger.sync_state().unwrap();
    assert!(!s.enabled);
    assert!(!s.degraded);

    let log_path = data_dir.join("runtab").join("cron.log");
    let log = std::fs::read_to_string(&log_path).expect("cron.log written");
    assert!(log.lines().any(|l| l.contains("skip:")), "expected a skip: line, got: {log}");
}
