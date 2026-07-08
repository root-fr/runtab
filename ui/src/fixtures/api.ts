import type {
  DailyResponse, Filters, HeatmapResponse, ModelsResponse, ModelTokens,
  PlanWindowResponse, PreviewRecordResponse, ProjectsResponse, ReviewProject,
  ReviewState, SavingsResponse, SessionRow, SessionsResponse, Settings, Summary,
  SyncedEvent, SyncStatus,
} from "@/api/types";
import {
  Bucket, DATASET, MACHINES, deletionHorizon, latestDay,
} from "./dataset";

const settings: Settings = {
  billing_mode_override: null,
  plan_limit_5h_tokens: 8_000_000,
  plan_limit_weekly_tokens: 100_000_000,
  machine_name: "laptop",
};

function tokensOf(b: Pick<ModelTokens, "input_tokens" | "output_tokens" | "cache_read_tokens" | "cache_creation_tokens" | "reasoning_tokens">): number {
  return b.input_tokens + b.output_tokens + b.cache_read_tokens + b.cache_creation_tokens + b.reasoning_tokens;
}

function applyFilters(rows: Bucket[], f: Filters): Bucket[] {
  return rows.filter((r) =>
    (!f.project || r.project_label === f.project) &&
    (!f.machine || r.machine_id === f.machine) &&
    (!f.from || r.date >= f.from) &&
    (!f.to || r.date <= f.to));
}

function emptyModel(model: string): ModelTokens & { total_tokens: number } {
  return {
    model, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0,
    cache_creation_tokens: 0, reasoning_tokens: 0, total_tokens: 0, est_cost_microusd: 0,
  };
}

function addInto(acc: ModelTokens, b: Bucket): void {
  acc.input_tokens += b.input_tokens;
  acc.output_tokens += b.output_tokens;
  acc.cache_read_tokens += b.cache_read_tokens;
  acc.cache_creation_tokens += b.cache_creation_tokens;
  acc.reasoning_tokens += b.reasoning_tokens;
  acc.total_tokens += tokensOf(b);
  acc.est_cost_microusd += b.est_cost_microusd;
}

function billing(): { mode: "subscription" | "api"; label: string; applicable: boolean } {
  const mode = settings.billing_mode_override ?? "subscription";
  const label = mode === "api" ? "estimated spend" : "API-equivalent value";
  return { mode, label, applicable: mode !== "api" };
}

function summary(f: Filters): Summary {
  const rows = applyFilters(DATASET, f);
  const today = latestDay();
  const bill = billing();
  const totals = rows.reduce(
    (a, b) => {
      a.tokens += tokensOf(b);
      a.cost += b.est_cost_microusd;
      a.cacheRead += b.cache_read_tokens;
      if (b.date === today) { a.todayTokens += tokensOf(b); a.todayCost += b.est_cost_microusd; }
      return a;
    },
    { tokens: 0, cost: 0, cacheRead: 0, todayTokens: 0, todayCost: 0 },
  );
  const since = rows.reduce((m, b) => (b.date < m ? b.date : m), today);
  return {
    total_tokens: totals.tokens,
    est_cost_microusd: totals.cost,
    cost_label: bill.label,
    billing_mode: bill.mode,
    plan_window_applicable: bill.applicable,
    today_tokens: totals.todayTokens,
    today_est_cost_microusd: totals.todayCost,
    cache_read_ratio: totals.tokens ? totals.cacheRead / totals.tokens : 0,
    project_count: new Set(rows.map((r) => r.project_label)).size,
    machine_count: new Set(rows.map((r) => r.machine_id)).size,
    since,
    modes: [{ billing_mode: bill.mode, est_cost_microusd: totals.cost, cost_label: bill.label }],
  };
}

function daily(f: Filters): DailyResponse {
  const rows = applyFilters(DATASET, f);
  const byDate = new Map<string, Map<string, ModelTokens>>();
  for (const b of rows) {
    const models = byDate.get(b.date) ?? new Map<string, ModelTokens>();
    const m = models.get(b.model) ?? emptyModel(b.model);
    addInto(m, b);
    models.set(b.model, m);
    byDate.set(b.date, models);
  }
  const days = [...byDate.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([date, models]) => {
      const by_model = [...models.values()].sort((a, b) => b.total_tokens - a.total_tokens);
      return {
        date,
        total_tokens: by_model.reduce((s, m) => s + m.total_tokens, 0),
        est_cost_microusd: by_model.reduce((s, m) => s + m.est_cost_microusd, 0),
        by_model,
      };
    });
  return { days };
}

function models(f: Filters): ModelsResponse {
  const rows = applyFilters(DATASET, f);
  const map = new Map<string, ModelTokens>();
  for (const b of rows) {
    const m = map.get(b.model) ?? emptyModel(b.model);
    addInto(m, b);
    map.set(b.model, m);
  }
  const grand = [...map.values()].reduce((s, m) => s + m.total_tokens, 0) || 1;
  return {
    models: [...map.values()]
      .sort((a, b) => b.total_tokens - a.total_tokens)
      .map((m) => ({ ...m, unpriced_events: 0, share: m.total_tokens / grand })),
  };
}

function projects(f: Filters): ProjectsResponse {
  const rows = applyFilters(DATASET, f);
  const map = new Map<string, { tokens: number; cost: number; sessions: Set<string> }>();
  for (const b of rows) {
    const p = map.get(b.project_label) ?? { tokens: 0, cost: 0, sessions: new Set<string>() };
    p.tokens += tokensOf(b);
    p.cost += b.est_cost_microusd;
    p.sessions.add(`${b.date}|${b.machine_id}`);
    map.set(b.project_label, p);
  }
  const grand = [...map.values()].reduce((s, p) => s + p.tokens, 0) || 1;
  return {
    projects: [...map.entries()]
      .sort(([, a], [, b]) => b.tokens - a.tokens)
      .map(([project_label, p]) => ({
        project_label, total_tokens: p.tokens, est_cost_microusd: p.cost,
        session_count: p.sessions.size, share: p.tokens / grand,
      })),
  };
}

let sessionCache: SessionRow[] | undefined;
function buildSessions(): SessionRow[] {
  if (sessionCache) return sessionCache;
  sessionCache = DATASET.map((b, i) => {
    const startHour = 8 + (i % 10);
    const durMin = 20 + (i % 7) * 18;
    const start = new Date(`${b.date}T${String(startHour).padStart(2, "0")}:05:00Z`);
    const end = new Date(start.getTime() + durMin * 60000);
    return {
      session_id: `sess-${b.date}-${b.machine_id}-${i}`,
      project_label: b.project_label,
      machine_name: b.machine_name,
      started_at: start.toISOString(),
      ended_at: end.toISOString(),
      model: b.model,
      event_count: 40 + (i % 12) * 17,
      total_tokens: tokensOf(b),
      est_cost_microusd: b.est_cost_microusd,
    };
  }).sort((a, b) => b.started_at.localeCompare(a.started_at));
  return sessionCache;
}

function sessions(f: Filters, page: number, pageSize: number): SessionsResponse {
  const all = buildSessions().filter((s) =>
    (!f.project || s.project_label === f.project) &&
    (!f.machine || MACHINES.find((m) => m.name === s.machine_name)?.id === f.machine) &&
    (!f.from || s.started_at.slice(0, 10) >= f.from) &&
    (!f.to || s.started_at.slice(0, 10) <= f.to));
  const startIdx = (page - 1) * pageSize;
  return { sessions: all.slice(startIdx, startIdx + pageSize), total: all.length, page, page_size: pageSize };
}

function heatmap(f: Filters): HeatmapResponse {
  const { days } = daily(f);
  const max = days.reduce((m, d) => Math.max(m, d.total_tokens), 0) || 1;
  return {
    days: days.map((d) => ({
      date: d.date, total_tokens: d.total_tokens, est_cost_microusd: d.est_cost_microusd,
      level: (d.total_tokens === 0 ? 0 : Math.min(4, 1 + Math.floor((d.total_tokens / max) * 4))) as 0 | 1 | 2 | 3 | 4,
    })),
    max_tokens: max,
    deletion_horizon: deletionHorizon(),
  };
}

// Deterministic per-day attributed savings as a small slice of that day's real
// consumption, so the fixture headline lands near the spec's ~2% example while
// the daily saved curve tracks the consumed one. Unattributed savings (grep
// noise that never reached a model) are surfaced only with no project/machine
// filter, mirroring the endpoint contract.
function savings(f: Filters): SavingsResponse {
  const { days } = daily(f);
  const daily_rows = days.map((d, i) => ({
    date: d.date,
    consumed: d.total_tokens,
    saved: Math.floor(d.total_tokens * (0.015 + ((i * 7) % 10) / 1000)),
  }));
  const consumed_tokens = daily_rows.reduce((s, d) => s + d.consumed, 0);
  const saved_attributed = daily_rows.reduce((s, d) => s + d.saved, 0);
  const scoped = !!(f.project || f.machine);
  return {
    window: {
      consumed_tokens,
      saved_attributed,
      saved_unattributed: scoped ? null : Math.floor(saved_attributed * 1.15),
      commands: daily_rows.length * 4,
      savings_ratio: consumed_tokens ? saved_attributed / consumed_tokens : null,
    },
    daily: daily_rows,
    rtk_present: true,
  };
}

function isoIn(ms: number): string {
  return new Date(Date.now() + ms).toISOString();
}

function planWindow(f: Filters): PlanWindowResponse {
  const bill = billing();
  if (!bill.applicable) {
    return { applicable: false, rolling_5h: null, weekly: null, limits_are_estimates: true };
  }
  const s = summary(f);
  const weekAgo = latestDay();
  const weekStart = new Date(`${weekAgo}T00:00:00Z`);
  weekStart.setUTCDate(weekStart.getUTCDate() - 7);
  const weeklyUsed = applyFilters(DATASET, f)
    .filter((b) => b.date >= weekStart.toISOString().slice(0, 10))
    .reduce((sum, b) => sum + tokensOf(b), 0);
  const rolling = Math.floor(s.today_tokens * 0.4);
  const nextMonday = new Date();
  nextMonday.setUTCHours(0, 0, 0, 0);
  nextMonday.setUTCDate(nextMonday.getUTCDate() + ((8 - nextMonday.getUTCDay()) % 7 || 7));
  return {
    applicable: true,
    rolling_5h: {
      window_hours: 5, tokens_used: rolling, limit_tokens: settings.plan_limit_5h_tokens,
      utilization: rolling / settings.plan_limit_5h_tokens, resets_at: isoIn(3.2 * 3600 * 1000),
    },
    weekly: {
      tokens_used: weeklyUsed, limit_tokens: settings.plan_limit_weekly_tokens,
      utilization: weeklyUsed / settings.plan_limit_weekly_tokens, resets_at: nextMonday.toISOString(),
    },
    limits_are_estimates: true,
  };
}

function syncStatus(): SyncStatus {
  return {
    enabled: true, state: "ok", account_email: "user@example.com", server_seq: 10432,
    pending_push: 0, last_push_at: isoIn(-60000), last_pull_at: isoIn(-90000), message: null,
    machines: MACHINES.map((m, i) => ({
      machine_id: m.id, machine_name: m.name, is_current: i === 0,
      last_seen: isoIn(-60000 * (i + 1)),
      event_count: DATASET.filter((b) => b.machine_id === m.id).length * 140,
    })),
  };
}

let reviewed = false;

function reviewProjects(): ReviewProject[] {
  const names = [...new Set(DATASET.map((b) => b.project_label))].sort();
  return names.map((name) => ({ name, label: null, excluded: false }));
}

// A whitelist-shaped sample: 64-hex ids the real server would accept, so the
// dev fixture drawer shows the same shape as production.
const previewSample: SyncedEvent = {
  event_id: "3f9a1c7e".repeat(8),
  ts: "2026-07-06T10:00:00Z",
  agent: "claude-code",
  model: "claude-sonnet-4",
  project_label: reviewProjects()[0]?.name ?? "tkm",
  session_id: "9b2e5d1a".repeat(8),
  machine_id: "6d1f0a2c-1b2c-4d5e-8f90-a1b2c3d4e5f6",
  machine_name: "laptop",
  input_tokens: 1024,
  output_tokens: 512,
  cache_read_tokens: 8000,
  cache_creation_tokens: 256,
  reasoning_tokens: 0,
  est_cost_microusd: 41230,
  cost_basis: "estimated",
};

const delay = <T,>(value: T): Promise<T> =>
  new Promise((resolve) => window.setTimeout(() => resolve(value), 220));

export const fixtureApi = {
  summary: (f: Filters = {}) => delay(summary(f)),
  daily: (f: Filters = {}) => delay(daily(f)),
  models: (f: Filters = {}) => delay(models(f)),
  projects: (f: Filters = {}) => delay(projects(f)),
  savings: (f: Filters = {}) => delay(savings(f)),
  sessions: (f: Filters = {}, page = 1, pageSize = 50) => delay(sessions(f, page, pageSize)),
  heatmap: (f: Filters = {}) => delay(heatmap(f)),
  planWindow: (f: Filters = {}) => delay(planWindow(f)),
  syncStatus: () => delay(syncStatus()),
  settings: () => delay({ ...settings }),
  updateSettings: (next: Settings) => {
    Object.assign(settings, next);
    return delay({ ...settings });
  },
  review: (): Promise<ReviewState> => delay({ reviewed, projects: reviewProjects() }),
  saveReview: (projects: ReviewProject[]): Promise<ReviewState> => {
    reviewed = true;
    return delay({ reviewed, projects });
  },
  previewRecord: (): Promise<PreviewRecordResponse> => delay({ record: previewSample }),
};
