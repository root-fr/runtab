use serde::{Deserialize, Serialize};

use crate::ledger::{Filter, HeatmapCell, MachineStat};

/// Global filters accepted by every `/api/*` endpoint as query params.
#[derive(Debug, Deserialize)]
pub struct FilterParams {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub agent: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl FilterParams {
    pub fn filter(self) -> Filter {
        Filter {
            project: clean(self.project),
            machine: clean(self.machine),
            agent: agent_source(self.agent),
            from: clean(self.from),
            to: clean(self.to),
        }
    }
}

/// `/api/tools` query params: independent of the global filters above (no
/// `project`/`machine`/date-range — just a day count and a session scope,
/// matching `Ledger::tool_aggregates`/`rtk_totals`).
#[derive(Debug, Deserialize)]
pub struct ToolsParams {
    pub days: Option<u32>,
    pub session: Option<String>,
}

impl ToolsParams {
    pub fn session(&self) -> Option<String> {
        clean(self.session.clone())
    }
}

/// Sessions endpoint pagination on top of the global filters.
#[derive(Debug, Deserialize)]
pub struct SessionParams {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub agent: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

impl SessionParams {
    pub fn filter(&self) -> Filter {
        Filter {
            project: clean(self.project.clone()),
            machine: clean(self.machine.clone()),
            agent: agent_source(self.agent.clone()),
            from: clean(self.from.clone()),
            to: clean(self.to.clone()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HeatmapResponse {
    pub days: Vec<HeatmapCell>,
    pub max_tokens: u64,
    pub deletion_horizon: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub enabled: bool,
    pub state: String,
    pub account_email: Option<String>,
    pub server_seq: i64,
    pub pending_push: u64,
    pub last_push_at: Option<String>,
    pub last_pull_at: Option<String>,
    pub message: Option<String>,
    pub machines: Vec<MachineStat>,
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Fold the hyphenated wire agent id (`claude-code`) to the local `source`
/// column form (`claude_code`) once at the DTO boundary.
fn agent_source(v: Option<String>) -> Option<String> {
    clean(v).map(|s| s.replace('-', "_"))
}
