/// Cost provenance for a usage event. First-class from day one so a later
/// vendor-API reconciliation is a data update, not a schema migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostBasis {
    Estimated,
    Logged,
    Billed,
}

impl CostBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            CostBasis::Estimated => "estimated",
            CostBasis::Logged => "logged",
            CostBasis::Billed => "billed",
        }
    }
}

/// A normalized usage event, mirroring the `usage_events` ledger schema.
///
/// Token fields are a superset of the published Claude Code shape: cache
/// creation is kept both as a total (`cache_creation_tokens`) and split into
/// its ephemeral 1h/5m components so cost can price each tier correctly.
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub source: String,
    pub message_id: String,
    pub request_id: String,
    pub session_id: String,
    pub ts: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_1h_tokens: i64,
    pub cache_5m_tokens: i64,
    pub reasoning_tokens: i64,
    pub project: String,
    pub agent_version: String,
    pub cost_usd: Option<f64>,
    pub cost_basis: CostBasis,
}

impl UsageEvent {
    /// Total billable token count, used by the dedup keep-higher-total policy.
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_creation_tokens
            + self.reasoning_tokens
    }
}

/// A `tool_use` block seen in a transcript line: the agent asking to invoke a
/// tool. Mirrors the `pending_tool_calls` / `tool_events` ledger schema; kept
/// separate from `UsageEvent` since a line can carry both.
#[derive(Debug, Clone)]
pub struct ToolUseSeen {
    pub source: String,
    pub session_id: String,
    pub tool_use_id: String,
    pub ts: String,
    pub project: String,
    pub tool_name: String,
    pub est_args_tokens: i64,
    /// JSON array of per-segment head hashes (`cmdnorm::chain_head_hashes`),
    /// `Bash` only; `None` for every other tool.
    pub bash_head_hashes: Option<String>,
    /// JSON array of per-segment command hashes (`cmdnorm::chain_hashes`),
    /// `Bash` only; `None` for every other tool.
    pub bash_chain_hashes: Option<String>,
}

/// A `tool_result` block seen in a transcript line: the outcome of a
/// previously requested `ToolUseSeen`, paired by `tool_use_id`.
#[derive(Debug, Clone)]
pub struct ToolResultSeen {
    pub source: String,
    pub session_id: String,
    pub tool_use_id: String,
    pub ts: String,
    pub est_result_tokens: i64,
    pub is_error: bool,
}

/// One row read from rtk's `commands` table, adapted for import into
/// `rtk_events`. `original_cmd` is deliberately not a field here: it is
/// hashed into `head_hash`/`cmd_hash` at read time and must never reach the
/// ledger as text.
#[derive(Debug, Clone)]
pub struct RtkCommandRow {
    pub rtk_row_id: i64,
    pub ts: String,
    pub project_path: String,
    pub head_hash: String,
    pub cmd_hash: String,
    pub raw_tokens: i64,
    pub filtered_tokens: i64,
    pub saved_tokens: i64,
    pub exec_time_ms: i64,
}

/// One `rtk_events` row still awaiting attribution (`match_kind = 'none'`),
/// as read back for the attribution pass. See `crate::rtkimport::attribute`.
#[derive(Debug, Clone)]
pub struct UnattributedRtkRow {
    pub id: i64,
    pub ts: String,
    pub project_path: String,
    pub head_hash: String,
    pub cmd_hash: String,
}

/// One `Bash` `tool_events` row inside an attribution candidate window. See
/// `crate::rtkimport::attribute`.
#[derive(Debug, Clone)]
pub struct BashCandidate {
    pub id: i64,
    pub source: String,
    pub session_id: String,
    pub project: String,
    pub ts: String,
    pub bash_head_hashes: Option<String>,
    pub bash_chain_hashes: Option<String>,
}
