import type { Summary } from "@/api/types";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { formatCost, formatPercent, formatTokens } from "@/lib/format";

export type HeroMetric = "tokens" | "cost" | "burn" | "cache";

interface KpiTilesProps {
  summary: Summary | undefined;
  loading: boolean;
  active: HeroMetric;
  onSelect: (metric: HeroMetric) => void;
}

interface TileDef {
  metric: HeroMetric;
  label: string;
  value: (s: Summary) => string;
  sub: (s: Summary) => string;
}

// Clicking a tile swaps the hero metric; the active tile is highlighted with
// the single accent so the link between tile and chart is unambiguous.
const TILES: TileDef[] = [
  {
    metric: "tokens",
    label: "Total tokens",
    value: (s) => formatTokens(s.total_tokens),
    sub: (s) => `${s.machine_count} ${s.machine_count === 1 ? "machine" : "machines"}`,
  },
  {
    metric: "cost",
    label: "value",
    value: (s) => formatCost(s.est_cost_microusd),
    sub: (s) => s.cost_label,
  },
  {
    metric: "burn",
    label: "Today's burn",
    value: (s) => formatCost(s.today_est_cost_microusd),
    sub: (s) => `${formatTokens(s.today_tokens)} tokens today`,
  },
  {
    metric: "cache",
    label: "Cache-read ratio",
    value: (s) => formatPercent(s.cache_read_ratio),
    sub: () => "of all tokens",
  },
];

export function KpiTiles({ summary, loading, active, onSelect }: KpiTilesProps) {
  return (
    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      {TILES.map((tile) => {
        const isActive = tile.metric === active;
        const label = tile.metric === "cost" && summary ? summary.cost_label : tile.label;
        return (
          <button
            key={tile.metric}
            onClick={() => onSelect(tile.metric)}
            aria-pressed={isActive}
            className={cn(
              "group flex flex-col gap-1 rounded-lg border p-4 text-left transition-colors duration-150",
              isActive
                ? "border-accent/60 bg-accent/10"
                : "border-border bg-card hover:border-accent/40 hover:bg-secondary/40",
            )}
          >
            <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {label}
            </span>
            {loading || !summary ? (
              <Skeleton className="mt-1 h-7 w-20" />
            ) : (
              <>
                <span className="text-2xl font-semibold tabular-nums text-foreground">
                  {tile.value(summary)}
                </span>
                <span className="truncate text-xs text-muted-foreground">{tile.sub(summary)}</span>
              </>
            )}
          </button>
        );
      })}
    </div>
  );
}
