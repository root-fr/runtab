// Types generated from docs/api/contract.md (v1). Keep in sync with that file;
// it is the authoritative interface for the local dashboard (/api) and the
// sync server (/v1). All costs are integer micro-USD (USD * 1e6).

export type CostBasis = "estimated" | "logged" | "billed";
export type BillingMode = "subscription" | "api" | "mixed";
export type SyncState = "ok" | "degraded" | "off";

// The fail-closed synced record (server rejects any extra field).
export interface SyncedEvent {
  event_id: string;
  ts: string;
  agent: string;
  model: string;
  project_label: string;
  session_id: string;
  machine_id: string;
  machine_name: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  reasoning_tokens: number;
  est_cost_microusd: number;
  cost_basis: CostBasis;
}

// Global filters accepted by every /api/* endpoint.
export interface Filters {
  project?: string;
  machine?: string;
  agent?: string;
  from?: string;
  to?: string;
}

export interface ApiErrorBody {
  error: string;
  detail: string | null;
}

// --- /api/summary ---------------------------------------------------------
export interface ModeCost {
  billing_mode: Exclude<BillingMode, "mixed">;
  est_cost_microusd: number;
  cost_label: string;
}

export interface Summary {
  total_tokens: number;
  est_cost_microusd: number;
  cost_label: string;
  billing_mode: BillingMode;
  plan_window_applicable: boolean;
  today_tokens: number;
  today_est_cost_microusd: number;
  cache_read_ratio: number;
  project_count: number;
  machine_count: number;
  // Null when the selection matches zero events (MIN over an empty set).
  since: string | null;
  modes: ModeCost[];
}

// --- /api/daily -----------------------------------------------------------
export interface ModelTokens {
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
  est_cost_microusd: number;
}

export interface DailyBucket {
  date: string;
  total_tokens: number;
  est_cost_microusd: number;
  by_model: ModelTokens[];
}

export interface DailyResponse {
  days: DailyBucket[];
}

// --- /api/models ----------------------------------------------------------
export interface ModelShare extends ModelTokens {
  unpriced_events: number;
  share: number;
}

export interface ModelsResponse {
  models: ModelShare[];
}

// --- /api/agents ----------------------------------------------------------
// Mirrors ModelShare, with `agent` (hyphenated id) replacing `model`. The
// endpoint wraps these in `{ agents: [...] }` (like `/api/models`), ranked by
// total tokens DESC; the client unwraps to this bare row array.
export interface AgentShare {
  agent: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
  est_cost_microusd: number;
  unpriced_events: number;
  share: number;
}

// --- /api/projects --------------------------------------------------------
export interface ProjectRow {
  project_label: string;
  total_tokens: number;
  est_cost_microusd: number;
  session_count: number;
  share: number;
}

export interface ProjectsResponse {
  projects: ProjectRow[];
}

// --- /api/sessions --------------------------------------------------------
export interface SessionRow {
  session_id: string;
  project_label: string;
  machine_name: string;
  agent: string;
  started_at: string;
  ended_at: string;
  model: string;
  event_count: number;
  total_tokens: number;
  est_cost_microusd: number;
}

export interface SessionsResponse {
  sessions: SessionRow[];
  total: number;
  page: number;
  page_size: number;
}

// --- /api/savings ---------------------------------------------------------
// rtk's savings against real consumption. `saved_unattributed` is null when a
// project, machine or agent filter is set (unattributed rows can't be scoped);
// `savings_ratio` is null when `consumed_tokens` is 0.
export interface SavingsWindow {
  consumed_tokens: number;
  saved_attributed: number;
  saved_unattributed: number | null;
  commands: number;
  savings_ratio: number | null;
}

export interface SavingsDaily {
  date: string;
  consumed: number;
  saved: number;
}

export interface SavingsResponse {
  window: SavingsWindow;
  daily: SavingsDaily[];
  rtk_present: boolean;
}

// --- /api/heatmap ---------------------------------------------------------
export interface HeatmapDay {
  date: string;
  total_tokens: number;
  est_cost_microusd: number;
  level: 0 | 1 | 2 | 3 | 4;
}

export interface HeatmapResponse {
  days: HeatmapDay[];
  max_tokens: number;
  deletion_horizon: string | null;
}

// --- /api/planwindow ------------------------------------------------------
// The weekly window carries no `window_hours` on the wire; only the rolling 5h
// one does. Kept as two shapes so the types match the server exactly.
export interface WeeklyWindow {
  tokens_used: number;
  limit_tokens: number;
  utilization: number;
  resets_at: string;
}

export interface PlanWindow extends WeeklyWindow {
  window_hours: number;
}

export interface PlanWindowResponse {
  applicable: boolean;
  rolling_5h: PlanWindow | null;
  weekly: WeeklyWindow | null;
  limits_are_estimates: boolean;
}

// --- /api/sync/status -----------------------------------------------------
export interface SyncMachine {
  machine_id: string;
  machine_name: string;
  is_current: boolean;
  last_seen: string | null;
  event_count: number;
}

export interface SyncStatus {
  enabled: boolean;
  state: SyncState;
  account_email: string | null;
  server_seq: number;
  pending_push: number;
  last_push_at: string | null;
  last_pull_at: string | null;
  message: string | null;
  machines: SyncMachine[];
}

// --- /api/settings --------------------------------------------------------
export interface Settings {
  billing_mode_override: Exclude<BillingMode, "mixed"> | null;
  plan_limit_5h_tokens: number;
  plan_limit_weekly_tokens: number;
  machine_name: string;
}

// --- /api/sync/review -----------------------------------------------------
// The pre-sync consent surface. `name` is the default basename label; `label`
// is the user's rename (null = keep the default); `excluded` drops it from sync.
export interface ReviewProject {
  name: string;
  label: string | null;
  excluded: boolean;
}

export interface ReviewState {
  reviewed: boolean;
  projects: ReviewProject[];
}

// --- /api/sync/preview-record ---------------------------------------------
// One real derived record, or null when the ledger is empty.
export interface PreviewRecordResponse {
  record: SyncedEvent | null;
}
