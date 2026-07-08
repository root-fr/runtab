// The UI chrome is a single blue accent. Data-viz series still need a
// categorical scale, so models get stable colors from a curated palette that
// leads with the brand blue/amber family and stays legible on white surfaces.
// Assignment is deterministic by model id so a model keeps its color across
// panels and reloads.

const MODEL_PALETTE: readonly string[] = [
  "#207bff", // brand blue
  "#e8a13a", // amber (runtab data-accent)
  "#2e9e6f", // sage
  "#7c3aed", // violet
  "#0891b2", // cyan
  "#db2777", // pink
  "#ea580c", // orange
  "#0059d4", // brand strong
  "#b45309", // amber deep
];

const MUTED = "#64748b";

function hashString(value: string): number {
  let h = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function modelColor(model: string): string {
  if (!model || model === "unknown") return MUTED;
  return MODEL_PALETTE[hashString(model) % MODEL_PALETTE.length];
}

// The five token kinds shown in cache-split tooltips and the cache hero view.
export const TOKEN_KIND_COLORS = {
  input: "#207bff",
  output: "#e8a13a",
  cache_read: "#0891b2",
  cache_creation: "#2e9e6f",
  reasoning: "#7c3aed",
} as const;

export type TokenKind = keyof typeof TOKEN_KIND_COLORS;

export const TOKEN_KIND_LABELS: Record<TokenKind, string> = {
  input: "Input",
  output: "Output",
  cache_read: "Cache read",
  cache_creation: "Cache write",
  reasoning: "Reasoning",
};
