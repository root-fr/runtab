use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use runtab::format::{fmt_count, fmt_noun, Style};
use runtab::ledger::{self, AggregateRow, Ledger, RtkTotals, ToolAggregateRow};
use runtab::pricing::Pricing;
use runtab::report;
use runtab::report::TableSpec;
use runtab::sync;
use runtab::timeutil;

#[derive(Parser)]
#[command(
    name = "runtab",
    version,
    about = "Local-first ledger for AI coding-agent token usage. Costs are estimates."
)]
struct Cli {
    /// Override the ledger database path.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Scan agent logs into the ledger (full backfill on first run).
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Token/cost totals per day (default: last 30 days).
    Daily {
        /// Machine-readable output; always full history, exact numbers.
        #[arg(long)]
        json: bool,
        /// Full history instead of the last 30 days.
        #[arg(long)]
        all: bool,
    },
    /// Token/cost totals per model.
    Models {
        #[arg(long)]
        json: bool,
    },
    /// Token/cost totals per project.
    Projects {
        #[arg(long)]
        json: bool,
    },
    /// Token/cost totals per session (default: last 30 days).
    Sessions {
        /// Machine-readable output; always full history, exact numbers.
        #[arg(long)]
        json: bool,
        /// Full history instead of the last 30 days.
        #[arg(long)]
        all: bool,
    },
    /// Estimated context tokens by tool-call type, plus rtk savings totals.
    Tools {
        #[arg(long)]
        json: bool,
    },
    /// Run the local dashboard (embedded SPA + local JSON API).
    Serve {
        /// Bind port (default 7822, auto-increment if busy; or RUNTAB_PORT).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Optional cloud sync across machines.
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
}

#[derive(Subcommand)]
enum SyncAction {
    /// Authorize this machine in a browser and enable sync.
    Login,
    /// Show sync state, account, and known machines.
    Status,
    /// Push and pull once now.
    Now,
    /// Disable sync on this machine (local data untouched).
    Off,
    /// Wipe the synced account on the server.
    Delete,
    /// Scheduled one-shot tick (scan + push/pull). Meant to be run from cron
    /// via `sync auto on`, not called directly.
    #[command(hide = true)]
    Run {
        #[arg(long)]
        verbose: bool,
    },
    /// Manage the crontab-scheduled background tick.
    Auto {
        #[command(subcommand)]
        action: AutoAction,
    },
}

#[derive(Subcommand)]
enum AutoAction {
    /// Install a managed crontab entry that runs `sync run` on a schedule.
    On {
        /// e.g. `30m` (default) or `2h`; must divide 60/24, floor 15m.
        #[arg(long)]
        interval: Option<String>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Remove the managed crontab entry.
    Off,
    /// Show whether auto-sync is installed and its last run.
    Status,
}

impl From<SyncAction> for sync::Cmd {
    fn from(a: SyncAction) -> sync::Cmd {
        match a {
            SyncAction::Login => sync::Cmd::Login,
            SyncAction::Status => sync::Cmd::Status,
            SyncAction::Now => sync::Cmd::Now,
            SyncAction::Off => sync::Cmd::Off,
            SyncAction::Delete => sync::Cmd::Delete,
            SyncAction::Run { verbose } => sync::Cmd::Run { verbose },
            SyncAction::Auto { action } => match action {
                AutoAction::On { interval, yes } => sync::Cmd::AutoOn { interval, yes },
                AutoAction::Off => sync::Cmd::AutoOff,
                AutoAction::Status => sync::Cmd::AutoStatus,
            },
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let db_path = match cli.db {
        Some(p) => p,
        None => ledger::default_db_path()?,
    };
    let ledger = Ledger::open(&db_path)?;
    let pricing = Pricing::load()?;
    let style = Style::detect();

    match cli.command {
        None => {
            let adapters = runtab::default_adapters();
            let db_adapters = runtab::default_db_adapters();
            let mut summary = scan_with_progress_line(&ledger, &adapters, &db_adapters, &pricing);
            summary.rtk = runtab::scan_rtk(&ledger);
            // The overview has no diagnostics section, so a scan that lost
            // events must not fail silently on the default entry point.
            if summary.db_errors > 0 {
                eprintln!(
                    "{}",
                    style.yellow(&format!(
                        "warning: {} during scan — run `runtab scan` for details",
                        fmt_noun(summary.db_errors as i64, "db error")
                    ))
                );
            }
            report::write_stdout(&runtab::overview::render(&ledger, &style)?);
        }
        Some(Command::Scan { json }) => {
            let was_empty = !json && ledger.totals(None)?.events == 0;
            let adapters = runtab::default_adapters();
            let db_adapters = runtab::default_db_adapters();
            let mut summary = scan_with_progress_line(&ledger, &adapters, &db_adapters, &pricing);
            summary.rtk = runtab::scan_rtk(&ledger);
            if json {
                report::print_json(&summary)?;
            } else {
                let totals = ledger.totals(None)?;
                report::write_stdout(&report::render_scan_summary(&summary, &totals, was_empty, &style));
            }
        }
        Some(Command::Daily { json, all }) => {
            emit_windowed(&ledger, json, all, "Daily usage", "DAY", |l, s| l.daily(s), &style)?
        }
        Some(Command::Models { json }) => {
            emit(&ledger.models(None)?, json, "Usage by model", "MODEL", &style)?
        }
        Some(Command::Projects { json }) => {
            emit(&ledger.projects(None)?, json, "Usage by project", "PROJECT", &style)?
        }
        Some(Command::Sessions { json, all }) => {
            emit_windowed(&ledger, json, all, "Usage by session", "SESSION", |l, s| l.sessions(s), &style)?
        }
        Some(Command::Tools { json }) => {
            let tools = ledger.tool_aggregates(None, None)?;
            let rtk = ledger.rtk_totals(None, None)?;
            if json {
                report::print_json(&ToolsReport { tools, rtk })?;
            } else {
                report::write_stdout(&report::render_tools_table(&tools, &style));
                if let Some(r) = &rtk {
                    report::write_stdout(&report::render_rtk_totals(r, &style));
                }
            }
        }
        Some(Command::Serve { port }) => runtab::serve::run(ledger, pricing, port)?,
        Some(Command::Sync { action }) => sync::dispatch(ledger, &pricing, &db_path, action.into())?,
    }
    Ok(())
}

/// JSON envelope for `runtab tools --json`: `{"tools": [...], "rtk": {...}|null}`.
#[derive(Serialize)]
struct ToolsReport {
    tools: Vec<ToolAggregateRow>,
    rtk: Option<RtkTotals>,
}

/// Runs a full scan (file + DB sources) with a single-line progress indicator on
/// stderr. TTY-only and throttled so piped/cron output stays clean and fast. Each
/// DB source counts as one progress unit, so opencode/hermes advance the bar the
/// same way a scanned file does — the count is dominated by transcript files, so
/// the label stays "files".
fn scan_with_progress_line(
    ledger: &Ledger,
    adapters: &[Box<dyn runtab::adapters::Adapter>],
    db_adapters: &[Box<dyn runtab::adapters::DbAdapter>],
    pricing: &Pricing,
) -> runtab::ScanSummary {
    use std::io::{IsTerminal, Write};
    let show = std::io::stderr().is_terminal();
    let mut printed = false;
    let mut cb = |done: u64, total: u64| {
        if show && (done.is_multiple_of(100) || done == total) {
            printed = true;
            eprint!(
                "\rScanning agent logs… {}/{} files",
                fmt_count(done as i64),
                fmt_count(total as i64)
            );
            let _ = std::io::stderr().flush();
        }
    };
    let summary = runtab::scan_all_with_progress(ledger, adapters, db_adapters, pricing, &mut cb);
    if printed {
        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
    }
    summary
}

fn emit(rows: &[AggregateRow], json: bool, title: &str, key_header: &str, style: &Style) -> anyhow::Result<()> {
    if json {
        report::print_json(rows)?;
    } else {
        let spec = TableSpec {
            title,
            key_header,
            empty_msg: report::empty_table_msg(false),
        };
        report::write_stdout(&report::render_table(&spec, rows, style));
    }
    Ok(())
}

/// Last-30-days lower bound, or `None` when `--all` lifts the window.
fn window(all: bool) -> Option<String> {
    (!all).then(|| timeutil::date_minus_days(&timeutil::today_utc(), 30))
}

fn emit_windowed(
    ledger: &Ledger,
    json: bool,
    all: bool,
    title: &str,
    key_header: &str,
    query: impl Fn(&Ledger, Option<&str>) -> rusqlite::Result<Vec<AggregateRow>>,
    style: &Style,
) -> anyhow::Result<()> {
    if json {
        // JSON keeps full history and exact numbers so scripts never see the
        // presentation window.
        report::print_json(&query(ledger, None)?)?;
        return Ok(());
    }
    let since = window(all);
    let rows = query(ledger, since.as_deref())?;
    let title_owned = if since.is_some() {
        format!("{title} (last 30 days)")
    } else {
        title.to_string()
    };
    let empty_msg = if rows.is_empty() {
        report::empty_table_msg(ledger.totals(None)?.events > 0)
    } else {
        ""
    };
    let spec = TableSpec { title: &title_owned, key_header, empty_msg };
    report::write_stdout(&report::render_table(&spec, &rows, style));
    Ok(())
}
