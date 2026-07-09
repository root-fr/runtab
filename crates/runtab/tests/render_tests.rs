use runtab::format::Style;
use runtab::ledger::{AggregateRow, Totals};
use runtab::report::{render_scan_summary, render_table, TableSpec};
use runtab::{RtkReport, ScanSummary};

fn row(key: &str, unpriced: i64, saved: Option<i64>) -> AggregateRow {
    AggregateRow {
        key: key.to_string(),
        events: 7_636,
        input_tokens: 6_670_786,
        output_tokens: 4_979_023,
        cache_read_tokens: 578_586_627,
        cache_creation_tokens: 50_985_273,
        total_tokens: 641_221_709,
        cost_usd: Some(1456.8404),
        unpriced_events: unpriced,
        saved_tokens: saved,
    }
}

fn spec<'a>() -> TableSpec<'a> {
    TableSpec {
        title: "Daily usage (last 30 days)",
        key_header: "DAY",
        empty_msg: "(no data yet — run 'runtab scan' to import your agent logs)",
    }
}

#[test]
fn render_table_humanizes_and_hides_zero_only_columns() {
    let out = render_table(&spec(), &[row("2026-07-01", 0, None)], &Style::new(false));
    assert!(out.starts_with("Daily usage (last 30 days)\n"));
    assert!(out.contains("DAY"));
    assert!(out.contains("CACHE RD"));
    assert!(out.contains("COST est"));
    assert!(!out.contains("UNPRICED"));
    assert!(!out.contains("SAVED"));
    assert!(out.contains("7,636"));
    assert!(out.contains("6.7M"));
    assert!(out.contains("578.6M"));
    assert!(out.contains("641.2M"));
    assert!(out.contains("$1,456.84"));
    // Columns align: every non-title line has the same width.
    let lines: Vec<&str> = out.lines().skip(1).collect();
    assert!(lines.len() >= 2);
    assert!(lines.iter().all(|l| l.chars().count() == lines[0].chars().count()));
}

#[test]
fn render_table_shows_unpriced_and_saved_when_present() {
    let rows = [row("2026-07-01", 3, Some(48_827_647))];
    let out = render_table(&spec(), &rows, &Style::new(false));
    assert!(out.contains("UNPRICED"));
    assert!(out.contains("SAVED"));
    assert!(out.contains("48.8M"));
}

#[test]
fn render_table_empty_prints_hint() {
    let out = render_table(&spec(), &[], &Style::new(false));
    assert_eq!(
        out,
        "Daily usage (last 30 days)\n  (no data yet — run 'runtab scan' to import your agent logs)\n"
    );
}

#[test]
fn render_table_colors_do_not_break_alignment() {
    let rows = [row("2026-07-01", 0, Some(400))];
    let plain = render_table(&spec(), &rows, &Style::new(false));
    let colored = render_table(&spec(), &rows, &Style::new(true));
    let strip = |s: &str| {
        let mut out = String::new();
        let mut in_esc = false;
        for c in s.chars() {
            match (in_esc, c) {
                (false, '\x1b') => in_esc = true,
                (false, _) => out.push(c),
                (true, 'm') => in_esc = false,
                (true, _) => {}
            }
        }
        out
    };
    assert_eq!(strip(&colored), plain);
}

fn totals(events: i64) -> Totals {
    Totals {
        events,
        total_tokens: 4_200_000_000,
        cost_usd: Some(14_900.09),
        unpriced_events: 0,
        sessions: 312,
        first_day: Some("2026-05-29".to_string()),
    }
}

#[test]
fn scan_summary_leads_with_payoff_and_hides_zero_diagnostics() {
    let s = ScanSummary {
        files_scanned: 10_382,
        events_inserted: 110,
        duplicates_dropped: 2_273,
        ..Default::default()
    };
    let out = render_scan_summary(&s, &totals(312), false, &Style::new(false));
    assert!(out.starts_with("Scan complete: 110 new events from 10,382 files (2,273 duplicates skipped)\n"));
    assert!(out.contains("Ledger: $14,900.09 est · 4.2B tokens · 312 sessions · since 2026-05-29"));
    assert!(!out.contains("db errors"));
    assert!(!out.contains("lines skipped"));
    assert!(!out.contains("pending tool calls"));
    assert!(!out.contains("Next:"));
}

#[test]
fn scan_summary_surfaces_nonzero_diagnostics_and_rtk() {
    let s = ScanSummary {
        files_scanned: 5,
        events_inserted: 1,
        db_errors: 2,
        lines_skipped: 7,
        pending_tool_calls: 40,
        rtk: Some(RtkReport {
            rows_imported: 150,
            attributed_text: 61,
            attributed_window: 2,
            unmatched: 3_443,
        }),
        ..Default::default()
    };
    let out = render_scan_summary(&s, &totals(312), false, &Style::new(false));
    assert!(out.contains("rtk savings: 150 commands imported, 63 attributed"));
    assert!(out.contains("warning: 7 lines skipped"));
    assert!(out.contains("warning: 2 db errors"));
    assert!(out.contains("warning: 40 pending tool calls"));
}

#[test]
fn scan_summary_first_run_shows_next_steps() {
    let s = ScanSummary { files_scanned: 10, events_inserted: 9, ..Default::default() };
    let out = render_scan_summary(&s, &totals(9), true, &Style::new(false));
    assert!(out.contains("Next: runtab daily · runtab models · runtab serve"));
}

#[test]
fn scan_summary_empty_ledger_says_so() {
    let s = ScanSummary::default();
    let empty = Totals {
        events: 0,
        total_tokens: 0,
        cost_usd: None,
        unpriced_events: 0,
        sessions: 0,
        first_day: None,
    };
    let out = render_scan_summary(&s, &empty, true, &Style::new(false));
    assert!(out.contains("Ledger: no agent usage recorded yet"));
    assert!(!out.contains("Next:"));
}

#[test]
fn scan_summary_uses_singular_nouns() {
    let s = ScanSummary {
        files_scanned: 1,
        events_inserted: 1,
        duplicates_dropped: 1,
        ..Default::default()
    };
    let out = render_scan_summary(&s, &totals(1), false, &Style::new(false));
    assert!(
        out.starts_with("Scan complete: 1 new event from 1 file (1 duplicate skipped)\n"),
        "got: {out}"
    );
}

#[test]
fn empty_table_msg_distinguishes_quiet_window_from_empty_ledger() {
    use runtab::report::empty_table_msg;
    assert_eq!(
        empty_table_msg(false),
        "(no data yet — run 'runtab scan' to import your agent logs)"
    );
    assert_eq!(
        empty_table_msg(true),
        "(no usage in the last 30 days — use --all for full history)"
    );
}
