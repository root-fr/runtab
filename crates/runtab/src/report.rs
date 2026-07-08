use serde::Serialize;

use crate::ledger::{AggregateRow, RtkTotals, ToolAggregateRow};
use crate::{RtkReport, ScanSummary};

const HEADERS: [&str; 9] = [
    "KEY", "EVENTS", "INPUT", "OUTPUT", "CACHE_R", "CACHE_C", "TOTAL", "COST(est)", "UNPRICED",
];
const SAVED_HEADER: &str = "SAVED";

const TOOL_HEADERS: [&str; 6] = ["TOOL", "CALLS", "EST_ARGS", "EST_RESULT", "EST_TOTAL", "SHARE"];

/// Serialize any value as machine-clean JSON on stdout.
pub fn print_json<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_scan_summary(s: &ScanSummary) {
    println!("Scan complete.");
    println!("  files scanned:      {}", s.files_scanned);
    println!("  events inserted:    {}", s.events_inserted);
    println!("  duplicates dropped: {}", s.duplicates_dropped);
    println!("  lines skipped:      {}", s.lines_skipped);
    println!("  db errors:          {}", s.db_errors);
    println!("  tool events:        {}", s.tool_events_inserted);
    println!("  pending tool calls: {}", s.pending_tool_calls);
    if s.unknown_models.is_empty() {
        println!("  unknown models:     0");
    } else {
        let list: Vec<&str> = s.unknown_models.iter().map(String::as_str).collect();
        println!(
            "  unknown models:     {} ({})",
            s.unknown_models.len(),
            list.join(", ")
        );
    }
}

pub fn print_rtk_report(r: &RtkReport) {
    println!(
        "rtk savings: {} imported, attributed {} text / {} window / {} unmatched",
        r.rows_imported, r.attributed_text, r.attributed_window, r.unmatched
    );
}

pub fn print_table(title: &str, rows: &[AggregateRow]) {
    println!("{title}");
    if rows.is_empty() {
        println!("  (no data)");
        return;
    }

    // Only widen the table with a SAVED column when rtk actually matched
    // something in this report — `daily`/`models` (always None) stay exactly
    // as before.
    let show_saved = rows.iter().any(|r| r.saved_tokens.is_some());
    let headers: Vec<String> = HEADERS
        .iter()
        .map(|s| s.to_string())
        .chain(show_saved.then(|| SAVED_HEADER.to_string()))
        .collect();
    let cells: Vec<Vec<String>> = rows.iter().map(|r| row_cells(r, show_saved)).collect();

    let mut widths: Vec<usize> = headers.iter().map(String::len).collect();
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    print_row(&headers, &widths);
    for row in &cells {
        print_row(row, &widths);
    }
}

fn row_cells(r: &AggregateRow, show_saved: bool) -> Vec<String> {
    let cost = match r.cost_usd {
        Some(c) => format!("${c:.4}"),
        None => "n/a".to_string(),
    };
    let mut cells = vec![
        r.key.clone(),
        r.events.to_string(),
        r.input_tokens.to_string(),
        r.output_tokens.to_string(),
        r.cache_read_tokens.to_string(),
        r.cache_creation_tokens.to_string(),
        r.total_tokens.to_string(),
        cost,
        r.unpriced_events.to_string(),
    ];
    if show_saved {
        cells.push(r.saved_tokens.map(|s| s.to_string()).unwrap_or_default());
    }
    cells
}

pub fn print_tools_table(rows: &[ToolAggregateRow]) {
    println!("Tool-call token usage");
    if rows.is_empty() {
        println!("  (no data)");
        return;
    }

    let headers: Vec<String> = TOOL_HEADERS.iter().map(|s| s.to_string()).collect();
    let cells: Vec<Vec<String>> = rows.iter().map(tool_row_cells).collect();

    let mut widths: Vec<usize> = headers.iter().map(String::len).collect();
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    print_row(&headers, &widths);
    for row in &cells {
        print_row(row, &widths);
    }
    println!("estimated context tokens (bytes/4), not billed tokens");
}

pub fn print_rtk_totals(r: &RtkTotals) {
    println!("rtk: {} commands, {} tokens saved (rtk's own estimate)", r.commands, r.saved_tokens);
}

fn tool_row_cells(r: &ToolAggregateRow) -> Vec<String> {
    vec![
        r.tool_name.clone(),
        r.calls.to_string(),
        r.est_args_tokens.to_string(),
        r.est_result_tokens.to_string(),
        r.est_total_tokens.to_string(),
        format!("{:.1}%", r.share_pct),
    ]
}

fn print_row(cells: &[String], widths: &[usize]) {
    let mut line = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let w = widths[i];
        if i == 0 {
            line.push_str(&format!("{cell:<w$}"));
        } else {
            line.push_str(&format!("  {cell:>w$}"));
        }
    }
    println!("{line}");
}
