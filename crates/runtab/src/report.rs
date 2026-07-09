use serde::Serialize;

use crate::format::{fmt_cost, fmt_count, fmt_noun, fmt_tokens, Style};
use crate::ledger::{AggregateRow, RtkTotals, ToolAggregateRow, Totals};
use crate::ScanSummary;

/// Write to stdout, treating a closed pipe (`… | head`) as a normal early
/// exit instead of the panic `print!` would raise.
pub fn write_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let res = out.write_all(s.as_bytes()).and_then(|()| out.flush());
    if let Err(e) = res {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("runtab: cannot write to stdout: {e}");
        std::process::exit(1);
    }
}

const NUMERIC_HEADERS: [&str; 7] =
    ["EVENTS", "INPUT", "OUTPUT", "CACHE RD", "CACHE WR", "TOTAL", "COST est"];

const TOOL_HEADERS: [&str; 6] = ["TOOL", "CALLS", "EST ARGS", "EST RESULT", "EST TOTAL", "SHARE"];

/// Serialize any value as machine-clean JSON on stdout.
pub fn print_json<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<()> {
    let mut s = serde_json::to_string_pretty(value)?;
    s.push('\n');
    write_stdout(&s);
    Ok(())
}

/// Post-scan report: payoff first, diagnostics only when nonzero, next-step
/// hints only on a first scan (`was_empty` = ledger had no events before).
pub fn render_scan_summary(
    s: &ScanSummary,
    totals: &Totals,
    was_empty: bool,
    style: &Style,
) -> String {
    let mut out = format!(
        "Scan complete: {} from {} ({} skipped)\n",
        fmt_noun(s.events_inserted as i64, "new event"),
        fmt_noun(s.files_scanned as i64, "file"),
        fmt_noun(s.duplicates_dropped as i64, "duplicate"),
    );
    if totals.events == 0 {
        out.push_str("Ledger: no agent usage recorded yet\n");
    } else {
        let since = totals
            .first_day
            .as_deref()
            .map(|d| format!(" · since {d}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "Ledger: {} est · {} tokens · {}{since}\n",
            fmt_cost(totals.cost_usd),
            fmt_tokens(totals.total_tokens),
            fmt_noun(totals.sessions, "session"),
        ));
    }
    if let Some(rtk) = &s.rtk {
        out.push_str(&format!(
            "rtk savings: {} imported, {} attributed\n",
            fmt_noun(rtk.rows_imported as i64, "command"),
            fmt_count((rtk.attributed_text + rtk.attributed_window) as i64),
        ));
    }
    for (n, noun, suffix) in [
        (s.lines_skipped, "line", " skipped"),
        (s.db_errors, "db error", ""),
        (s.pending_tool_calls, "pending tool call", ""),
    ] {
        if n > 0 {
            out.push_str(&style.yellow(&format!("warning: {}{suffix}", fmt_noun(n as i64, noun))));
            out.push('\n');
        }
    }
    if !s.unknown_models.is_empty() {
        let list: Vec<&str> = s.unknown_models.iter().map(String::as_str).collect();
        out.push_str(&style.yellow(&format!(
            "warning: {} ({})",
            fmt_noun(s.unknown_models.len() as i64, "unknown model"),
            list.join(", ")
        )));
        out.push('\n');
    }
    if was_empty && totals.events > 0 {
        out.push_str(&format!(
            "Next: {} · {} · {}\n",
            style.bold("runtab daily"),
            style.bold("runtab models"),
            style.bold("runtab serve"),
        ));
    }
    out
}

/// Empty-table hint: point at `scan` when the ledger holds nothing at all,
/// at `--all` when only the 30-day window is quiet.
pub fn empty_table_msg(ledger_has_events: bool) -> &'static str {
    if ledger_has_events {
        "(no usage in the last 30 days — use --all for full history)"
    } else {
        "(no data yet — run 'runtab scan' to import your agent logs)"
    }
}

/// Per-report labels for `render_table`; the caller owns title and empty text
/// because only it knows the report kind and whether a window applies.
pub struct TableSpec<'a> {
    pub title: &'a str,
    pub key_header: &'a str,
    pub empty_msg: &'a str,
}

pub fn render_table(spec: &TableSpec, rows: &[AggregateRow], style: &Style) -> String {
    let mut out = String::new();
    out.push_str(spec.title);
    out.push('\n');
    if rows.is_empty() {
        out.push_str("  ");
        out.push_str(spec.empty_msg);
        out.push('\n');
        return out;
    }

    // Data-quality columns appear only when they carry information, so the
    // common all-priced / no-rtk table stays narrow.
    let show_unpriced = rows.iter().any(|r| r.unpriced_events > 0);
    let show_saved = rows.iter().any(|r| r.saved_tokens.is_some());

    let mut headers: Vec<String> = std::iter::once(spec.key_header)
        .chain(NUMERIC_HEADERS)
        .map(str::to_string)
        .collect();
    if show_unpriced {
        headers.push("UNPRICED".to_string());
    }
    if show_saved {
        headers.push("SAVED".to_string());
    }

    let cells: Vec<Vec<String>> =
        rows.iter().map(|r| row_cells(r, show_unpriced, show_saved)).collect();
    let widths = column_widths(&headers, &cells);
    let saved_idx = show_saved.then(|| headers.len() - 1);

    out.push_str(&style.dim(&layout(&headers, &widths, None, style)));
    out.push('\n');
    for row in &cells {
        out.push_str(&layout(row, &widths, saved_idx, style));
        out.push('\n');
    }
    out
}

fn row_cells(r: &AggregateRow, show_unpriced: bool, show_saved: bool) -> Vec<String> {
    let mut cells = vec![
        r.key.clone(),
        fmt_count(r.events),
        fmt_tokens(r.input_tokens),
        fmt_tokens(r.output_tokens),
        fmt_tokens(r.cache_read_tokens),
        fmt_tokens(r.cache_creation_tokens),
        fmt_tokens(r.total_tokens),
        fmt_cost(r.cost_usd),
    ];
    if show_unpriced {
        cells.push(fmt_count(r.unpriced_events));
    }
    if show_saved {
        cells.push(r.saved_tokens.map(fmt_tokens).unwrap_or_default());
    }
    cells
}

fn column_widths(headers: &[String], cells: &[Vec<String>]) -> Vec<usize> {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    widths
}

// Pads first (against plain text), colors after, so ANSI escapes never skew
// column widths. `green_idx` highlights one column's non-empty values.
fn layout(cells: &[String], widths: &[usize], green_idx: Option<usize>, style: &Style) -> String {
    let mut line = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let w = widths[i];
        let padded = if i == 0 {
            format!("{cell:<w$}")
        } else {
            format!("  {cell:>w$}")
        };
        if Some(i) == green_idx && !cell.is_empty() {
            line.push_str(&style.green(&padded));
        } else {
            line.push_str(&padded);
        }
    }
    line
}

pub fn render_tools_table(rows: &[ToolAggregateRow], style: &Style) -> String {
    let mut out = String::from("Tool-call token usage\n");
    if rows.is_empty() {
        out.push_str("  (no data yet — run 'runtab scan' to import your agent logs)\n");
        return out;
    }
    let headers: Vec<String> = TOOL_HEADERS.iter().map(|s| s.to_string()).collect();
    let cells: Vec<Vec<String>> = rows.iter().map(tool_row_cells).collect();
    let widths = column_widths(&headers, &cells);
    out.push_str(&style.dim(&layout(&headers, &widths, None, style)));
    out.push('\n');
    for row in &cells {
        out.push_str(&layout(row, &widths, None, style));
        out.push('\n');
    }
    out.push_str(&style.dim("estimated context tokens (bytes/4), not billed tokens"));
    out.push('\n');
    out
}

fn tool_row_cells(r: &ToolAggregateRow) -> Vec<String> {
    vec![
        r.tool_name.clone(),
        fmt_count(r.calls),
        fmt_tokens(r.est_args_tokens),
        fmt_tokens(r.est_result_tokens),
        fmt_tokens(r.est_total_tokens),
        format!("{:.1}%", r.share_pct),
    ]
}

pub fn render_rtk_totals(r: &RtkTotals, style: &Style) -> String {
    format!(
        "rtk: {} commands, {} saved (rtk's own estimate)\n",
        fmt_count(r.commands),
        style.green(&format!("{} tokens", fmt_tokens(r.saved_tokens))),
    )
}
