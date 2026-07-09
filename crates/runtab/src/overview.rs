//! The bare-`runtab` overview: a 30-day snapshot plus command hints. Pure
//! rendering over the ledger — the caller decides whether to scan first.

use crate::format::{fmt_cost, fmt_count, fmt_tokens, Style};
use crate::ledger::{Ledger, Totals};
use crate::timeutil;

pub fn render(ledger: &Ledger, style: &Style) -> anyhow::Result<String> {
    let all = ledger.totals(None)?;
    let mut out = String::new();
    out.push_str(&style.bold("runtab — token ledger for AI coding agents"));
    out.push_str("\n\n");

    if all.events == 0 {
        out.push_str("No agent usage found. runtab scans Claude Code transcripts in:\n");
        out.push_str("  ~/.claude/projects  (plus $CLAUDE_CONFIG_DIR and XDG variants)\n\n");
        out.push_str("Use your agent, then run `runtab` again. `runtab --help` lists all commands.\n");
        return Ok(out);
    }

    let since = timeutil::date_minus_days(&timeutil::today_utc(), 30);
    let window = ledger.totals(Some(&since))?;
    let today = ledger.totals(Some(&timeutil::today_utc()))?;

    out.push_str(&row("Last 30 days", &totals_line(&window)));
    out.push_str(&row("Today", &totals_line(&today)));
    if let Some(top) = top_by_cost(ledger.models(Some(&since))?) {
        out.push_str(&row("Top model", &top_line(&top.key, top.cost_usd, window.cost_usd)));
    }
    if let Some(top) = top_by_cost(ledger.projects(Some(&since))?) {
        let name = top.key.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("(unknown)");
        out.push_str(&row("Top project", &top_line(name, top.cost_usd, window.cost_usd)));
    }
    out.push('\n');
    out.push_str(&format!("  {}      per-day breakdown\n", style.bold("runtab daily")));
    out.push_str(&format!("  {}     per-model breakdown\n", style.bold("runtab models")));
    out.push_str(&format!("  {}      local dashboard (browser)\n", style.bold("runtab serve")));
    out.push_str(&format!("  {}     all commands\n", style.bold("runtab --help")));
    Ok(out)
}

fn row(label: &str, value: &str) -> String {
    format!("  {label:<14} {value}\n")
}

// The line renders cost and cost share, so "top" must mean cost — a
// cache-heavy model can lead on tokens (the queries' sort order) while a
// pricier one dominates spend. Strict `>` keeps the query's token order as
// the tie-break for unpriced rows.
fn top_by_cost(rows: Vec<crate::ledger::AggregateRow>) -> Option<crate::ledger::AggregateRow> {
    let mut best: Option<crate::ledger::AggregateRow> = None;
    for r in rows {
        let better = match &best {
            None => true,
            Some(b) => r.cost_usd.unwrap_or(0.0) > b.cost_usd.unwrap_or(0.0),
        };
        if better {
            best = Some(r);
        }
    }
    best
}

fn totals_line(t: &Totals) -> String {
    if t.events == 0 {
        return "—".to_string();
    }
    format!(
        "{} est · {} tokens · {} sessions",
        fmt_cost(t.cost_usd),
        fmt_tokens(t.total_tokens),
        fmt_count(t.sessions),
    )
}

// "model-y · $6.00 (75%)" — share of the window's priced cost, when both are
// known and nonzero.
fn top_line(name: &str, cost: Option<f64>, window_cost: Option<f64>) -> String {
    let mut s = name.to_string();
    if let Some(c) = cost {
        s.push_str(&format!(" · {}", fmt_cost(Some(c))));
        if let Some(w) = window_cost {
            if w > 0.0 {
                s.push_str(&format!(" ({:.0}%)", 100.0 * c / w));
            }
        }
    }
    s
}
