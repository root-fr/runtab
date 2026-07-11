// Typed client for the local dashboard API (/api/*). Generated from
// docs/api/contract.md (v1). The dashboard SPA is the only caller; the sync
// server (/v1) is driven by the Rust client, not this app.
//
// VITE_FIXTURES=1 swaps in a synthetic backend via dynamic import, so the UI
// runs with no server and the fixtures code is dropped from a normal build.

import type {
  AgentShare, DailyResponse, Filters, HeatmapResponse, ModelsResponse,
  PlanWindowResponse, PreviewRecordResponse, ProjectsResponse, ReviewProject,
  ReviewState, SavingsResponse, SessionsResponse, Settings, Summary, SyncStatus,
} from "./types";

const API_BASE = import.meta.env.VITE_API_BASE ?? "";

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, detail: string | null) {
    super(detail ?? code);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

function buildQuery(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== "") search.set(key, String(value));
  }
  const qs = search.toString();
  return qs ? `?${qs}` : "";
}

async function toApiError(response: Response): Promise<ApiError> {
  try {
    const body = (await response.json()) as { error?: string; detail?: string | null };
    return new ApiError(response.status, body.error ?? "http_error", body.detail ?? null);
  } catch {
    return new ApiError(response.status, "http_error", response.statusText || null);
  }
}

async function getJson<T>(
  path: string,
  params: Record<string, string | number | undefined> = {},
): Promise<T> {
  const response = await fetch(`${API_BASE}${path}${buildQuery(params)}`, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) throw await toApiError(response);
  return (await response.json()) as T;
}

async function putJson<T>(path: string, body: T): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await toApiError(response);
  return (await response.json()) as T;
}

async function postJson<Req, Res>(path: string, body: Req): Promise<Res> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await toApiError(response);
  return (await response.json()) as Res;
}

function filterParams(filters: Filters): Record<string, string | undefined> {
  return {
    project: filters.project, machine: filters.machine, agent: filters.agent,
    from: filters.from, to: filters.to,
  };
}

export interface DashboardApi {
  summary(filters?: Filters): Promise<Summary>;
  daily(filters?: Filters): Promise<DailyResponse>;
  models(filters?: Filters): Promise<ModelsResponse>;
  agents(filters?: Filters): Promise<AgentShare[]>;
  projects(filters?: Filters): Promise<ProjectsResponse>;
  savings(filters?: Filters): Promise<SavingsResponse>;
  sessions(filters?: Filters, page?: number, pageSize?: number): Promise<SessionsResponse>;
  heatmap(filters?: Filters): Promise<HeatmapResponse>;
  planWindow(filters?: Filters): Promise<PlanWindowResponse>;
  syncStatus(): Promise<SyncStatus>;
  settings(): Promise<Settings>;
  updateSettings(next: Settings): Promise<Settings>;
  review(): Promise<ReviewState>;
  saveReview(projects: ReviewProject[]): Promise<ReviewState>;
  previewRecord(): Promise<PreviewRecordResponse>;
}

const httpApi: DashboardApi = {
  summary: (f = {}) => getJson<Summary>("/api/summary", filterParams(f)),
  daily: (f = {}) => getJson<DailyResponse>("/api/daily", filterParams(f)),
  models: (f = {}) => getJson<ModelsResponse>("/api/models", filterParams(f)),
  agents: (f = {}) =>
    getJson<{ agents: AgentShare[] }>("/api/agents", filterParams(f)).then((b) => b.agents),
  projects: (f = {}) => getJson<ProjectsResponse>("/api/projects", filterParams(f)),
  savings: (f = {}) => getJson<SavingsResponse>("/api/savings", filterParams(f)),
  sessions: (f = {}, page = 1, pageSize = 50) =>
    getJson<SessionsResponse>("/api/sessions", { ...filterParams(f), page, page_size: pageSize }),
  heatmap: (f = {}) => getJson<HeatmapResponse>("/api/heatmap", filterParams(f)),
  planWindow: (f = {}) => getJson<PlanWindowResponse>("/api/planwindow", filterParams(f)),
  syncStatus: () => getJson<SyncStatus>("/api/sync/status"),
  settings: () => getJson<Settings>("/api/settings"),
  updateSettings: (next) => putJson<Settings>("/api/settings", next),
  review: () => getJson<ReviewState>("/api/sync/review"),
  saveReview: (projects) => postJson<{ projects: ReviewProject[] }, ReviewState>("/api/sync/review", { projects }),
  previewRecord: () => getJson<PreviewRecordResponse>("/api/sync/preview-record"),
};

let backendPromise: Promise<DashboardApi> | null = null;
function backend(): Promise<DashboardApi> {
  if (!backendPromise) {
    backendPromise = import.meta.env.VITE_FIXTURES === "1"
      ? import("@/fixtures/api").then((m) => m.fixtureApi)
      : Promise.resolve(httpApi);
  }
  return backendPromise;
}

export const api: DashboardApi = {
  summary: (f) => backend().then((b) => b.summary(f)),
  daily: (f) => backend().then((b) => b.daily(f)),
  models: (f) => backend().then((b) => b.models(f)),
  agents: (f) => backend().then((b) => b.agents(f)),
  projects: (f) => backend().then((b) => b.projects(f)),
  savings: (f) => backend().then((b) => b.savings(f)),
  sessions: (f, page, pageSize) => backend().then((b) => b.sessions(f, page, pageSize)),
  heatmap: (f) => backend().then((b) => b.heatmap(f)),
  planWindow: (f) => backend().then((b) => b.planWindow(f)),
  syncStatus: () => backend().then((b) => b.syncStatus()),
  settings: () => backend().then((b) => b.settings()),
  updateSettings: (next) => backend().then((b) => b.updateSettings(next)),
  review: () => backend().then((b) => b.review()),
  saveReview: (projects) => backend().then((b) => b.saveReview(projects)),
  previewRecord: () => backend().then((b) => b.previewRecord()),
};
