import type { DailyBucket } from "@/api/types";
import { modelColor, TOKEN_KIND_COLORS, TOKEN_KIND_LABELS, type TokenKind } from "@/lib/colors";
import type { HeroMetric } from "@/components/recap/KpiTiles";

export interface Segment {
  key: string;
  label: string;
  color: string;
  value: number;
}

const KIND_ORDER: TokenKind[] = ["input", "output", "cache_read", "cache_creation", "reasoning"];

// Global model order (by total tokens, desc) so a model keeps the same stacking
// position and colour across every day column.
export function collectModels(days: DailyBucket[]): string[] {
  const totals = new Map<string, number>();
  for (const day of days) {
    for (const m of day.by_model) {
      totals.set(m.model, (totals.get(m.model) ?? 0) + m.total_tokens);
    }
  }
  return [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([model]) => model);
}

function kindValue(m: DailyBucket["by_model"][number], kind: TokenKind): number {
  return m[`${kind}_tokens` as const];
}

export function dayKindTotals(day: DailyBucket): Record<TokenKind, number> {
  const out = { input: 0, output: 0, cache_read: 0, cache_creation: 0, reasoning: 0 };
  for (const m of day.by_model) {
    for (const kind of KIND_ORDER) out[kind] += kindValue(m, kind);
  }
  return out;
}

// Segments for one day column. Model metrics stack by model; the cache view
// stacks the five token kinds so the cache-read share is visible at a glance.
export function daySegments(day: DailyBucket, metric: HeroMetric, modelOrder: string[]): Segment[] {
  if (metric === "cache") {
    const totals = dayKindTotals(day);
    return KIND_ORDER.map((kind) => ({
      key: kind,
      label: TOKEN_KIND_LABELS[kind],
      color: TOKEN_KIND_COLORS[kind],
      value: totals[kind],
    }));
  }
  const byModel = new Map(day.by_model.map((m) => [m.model, m]));
  return modelOrder.map((model) => {
    const m = byModel.get(model);
    const value = !m ? 0 : metric === "cost" ? m.est_cost_microusd : m.total_tokens;
    return { key: model, label: model, color: modelColor(model), value };
  });
}

export interface LegendItem {
  key: string;
  label: string;
  color: string;
}

export function heroLegend(metric: HeroMetric, modelOrder: string[]): LegendItem[] {
  if (metric === "cache") {
    return KIND_ORDER.map((kind) => ({
      key: kind,
      label: TOKEN_KIND_LABELS[kind],
      color: TOKEN_KIND_COLORS[kind],
    }));
  }
  return modelOrder.map((model) => ({ key: model, label: model, color: modelColor(model) }));
}
