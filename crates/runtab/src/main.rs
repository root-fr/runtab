use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;

use runtab::ledger::{self, AggregateRow, Ledger, RtkTotals, ToolAggregateRow};
use runtab::pricing::Pricing;
use runtab::report;
use runtab::sync;

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
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan agent logs into the ledger (full backfill on first run).
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Token/cost totals per day.
    Daily {
        #[arg(long)]
        json: bool,
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
    /// Token/cost totals per session.
    Sessions {
        #[arg(long)]
        json: bool,
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

    match cli.command {
        Command::Scan { json } => {
            let adapters = runtab::default_adapters();
            let mut summary = runtab::scan(&ledger, &adapters, &pricing);
            summary.rtk = runtab::scan_rtk(&ledger);
            if json {
                report::print_json(&summary)?;
            } else {
                report::print_scan_summary(&summary);
                if let Some(rtk) = &summary.rtk {
                    report::print_rtk_report(rtk);
                }
            }
        }
        Command::Daily { json } => emit(&ledger.daily()?, json, "Daily usage")?,
        Command::Models { json } => emit(&ledger.models()?, json, "Usage by model")?,
        Command::Projects { json } => emit(&ledger.projects()?, json, "Usage by project")?,
        Command::Sessions { json } => emit(&ledger.sessions()?, json, "Usage by session")?,
        Command::Tools { json } => {
            let tools = ledger.tool_aggregates(None, None)?;
            let rtk = ledger.rtk_totals(None, None)?;
            if json {
                report::print_json(&ToolsReport { tools, rtk })?;
            } else {
                report::print_tools_table(&tools);
                if let Some(r) = &rtk {
                    report::print_rtk_totals(r);
                }
            }
        }
        Command::Serve { port } => runtab::serve::run(ledger, pricing, port)?,
        Command::Sync { action } => sync::dispatch(ledger, &pricing, &db_path, action.into())?,
    }
    Ok(())
}

/// JSON envelope for `runtab tools --json`: `{"tools": [...], "rtk": {...}|null}`.
#[derive(Serialize)]
struct ToolsReport {
    tools: Vec<ToolAggregateRow>,
    rtk: Option<RtkTotals>,
}

fn emit(rows: &[AggregateRow], json: bool, title: &str) -> anyhow::Result<()> {
    if json {
        report::print_json(rows)?;
    } else {
        report::print_table(title, rows);
    }
    Ok(())
}
