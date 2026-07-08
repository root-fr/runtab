import type { Filters } from "@/api/types";

export interface RangePreset {
  id: string;
  label: string;
  from?: string;
  to?: string;
}

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

function minusDays(n: number): string {
  const d = new Date();
  d.setUTCHours(0, 0, 0, 0);
  d.setUTCDate(d.getUTCDate() - n);
  return d.toISOString().slice(0, 10);
}

export function rangePresets(): RangePreset[] {
  const to = today();
  return [
    { id: "all", label: "All time" },
    { id: "7", label: "Last 7 days", from: minusDays(6), to },
    { id: "30", label: "Last 30 days", from: minusDays(29), to },
    { id: "90", label: "Last 90 days", from: minusDays(89), to },
    { id: "ytd", label: "This year", from: `${new Date().getUTCFullYear()}-01-01`, to },
    { id: "custom", label: "Custom range" },
  ];
}

export function activePresetId(filters: Filters): string {
  if (!filters.from && !filters.to) return "all";
  for (const preset of rangePresets()) {
    if (preset.id === "all" || preset.id === "custom") continue;
    if (preset.from === filters.from && preset.to === filters.to) return preset.id;
  }
  return "custom";
}
