import type { CostBasis } from "@/api/types";

// Deterministic synthetic dataset for VITE_FIXTURES=1 (runs the UI with no
// backend). A seeded LCG keeps every reload identical so panels and totals are
// stable to eyeball against. Anchored to the real current day so "today" and the
// T-30d deletion horizon land where a first-time user would expect.

export interface Bucket {
  date: string;
  agent: string;
  model: string;
  project_label: string;
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

interface Machine {
  id: string;
  name: string;
}

export const MACHINES: Machine[] = [
  { id: "mac-laptop-6d1f", name: "laptop" },
  { id: "mac-desktop-aa02", name: "desktop" },
];

const MODELS = ["claude-opus-4", "claude-sonnet-4", "claude-haiku-3.5"];
const PROJECTS = ["tkm", "ledger-web", "infra-scripts", "notes-cli"];

// Hyphenated agent ids, weighted so the mix reads as a mostly-Claude user who
// occasionally reaches for Codex or opencode.
const AGENTS = [
  "claude-code", "claude-code", "claude-code", "claude-code",
  "codex", "codex", "opencode",
];

// micro-USD per 1M tokens, blended input/output. Coarse, for shape not billing.
const MODEL_RATE: Record<string, number> = {
  "claude-opus-4": 9_000_000,
  "claude-sonnet-4": 1_800_000,
  "claude-haiku-3.5": 250_000,
};

const HISTORY_DAYS = 118;

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 0xffffffff;
  };
}

function utcDay(offsetDays: number): string {
  const d = new Date();
  d.setUTCHours(0, 0, 0, 0);
  d.setUTCDate(d.getUTCDate() - offsetDays);
  return d.toISOString().slice(0, 10);
}

export function latestDay(): string {
  return utcDay(0);
}

export function deletionHorizon(): string {
  return utcDay(30);
}

function buildBucket(
  rand: () => number,
  date: string,
  agent: string,
  model: string,
  project: string,
  machine: Machine,
  weight: number,
): Bucket {
  const base = Math.floor((4000 + rand() * 60000) * weight);
  const cache_read = Math.floor(base * (6 + rand() * 8));
  const cache_creation = Math.floor(base * (0.2 + rand() * 0.4));
  const output = Math.floor(base * (0.3 + rand() * 0.5));
  const reasoning = model === "claude-opus-4" ? Math.floor(base * rand() * 0.3) : 0;
  const billable = base + output + reasoning + Math.floor(cache_read * 0.1);
  const est_cost_microusd = Math.floor((billable / 1e6) * MODEL_RATE[model]);
  return {
    date,
    agent,
    model,
    project_label: project,
    machine_id: machine.id,
    machine_name: machine.name,
    input_tokens: base,
    output_tokens: output,
    cache_read_tokens: cache_read,
    cache_creation_tokens: cache_creation,
    reasoning_tokens: reasoning,
    est_cost_microusd,
    cost_basis: "estimated",
  };
}

// A gentle upward ramp with weekly troughs so the hero bars and heatmap read as
// a real working cadence rather than noise.
function dayWeight(rand: () => number, dayIndex: number, weekday: number): number {
  const trend = 0.5 + dayIndex / HISTORY_DAYS;
  const weekend = weekday === 0 || weekday === 6 ? 0.35 : 1;
  return trend * weekend * (0.6 + rand() * 0.8);
}

function generate(): Bucket[] {
  const rand = lcg(0x51ed23);
  const out: Bucket[] = [];
  for (let i = HISTORY_DAYS; i >= 0; i -= 1) {
    const date = utcDay(i);
    const weekday = new Date(`${date}T00:00:00Z`).getUTCDay();
    const dayIndex = HISTORY_DAYS - i;
    for (const machine of MACHINES) {
      // desktop only starts contributing partway through (later signup).
      if (machine.id === MACHINES[1].id && i > 74) continue;
      const activeProjects = machine.id === MACHINES[0].id
        ? PROJECTS.slice(0, 3)
        : PROJECTS.slice(1);
      for (const project of activeProjects) {
        if (rand() < 0.45) continue;
        const model = MODELS[Math.floor(rand() * MODELS.length)];
        const agent = AGENTS[Math.floor(rand() * AGENTS.length)];
        const weight = dayWeight(rand, dayIndex, weekday);
        if (weight <= 0) continue;
        out.push(buildBucket(rand, date, agent, model, project, machine, weight));
      }
    }
  }
  return out;
}

export const DATASET: Bucket[] = generate();
