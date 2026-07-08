import type { DailyBucket } from "@/api/types";
import { TOKEN_KIND_COLORS, TOKEN_KIND_LABELS, type TokenKind } from "@/lib/colors";
import { formatCost, formatMonthDay, formatTokens } from "@/lib/format";
import type { HeroMetric } from "@/components/recap/KpiTiles";
import { dayKindTotals } from "./heroData";

interface HeroTooltipProps {
  day: DailyBucket;
  metric: HeroMetric;
  left: number;
  flip: boolean;
}

const KINDS: TokenKind[] = ["input", "output", "cache_read", "cache_creation", "reasoning"];

// Floating tooltip. Always carries the cache split (all five token kinds) so the
// cache economics are legible regardless of which hero metric is selected.
export function HeroTooltip({ day, metric, left, flip }: HeroTooltipProps) {
  const kinds = dayKindTotals(day);
  const total = metric === "cost" ? formatCost(day.est_cost_microusd) : formatTokens(day.total_tokens);
  return (
    <div
      className="pointer-events-none absolute top-2 z-20 w-56 rounded-lg border border-border bg-popover/95 p-3 text-xs shadow-xl backdrop-blur"
      style={{ left, transform: flip ? "translateX(-100%)" : undefined }}
    >
      <div className="mb-2 flex items-center justify-between">
        <span className="font-semibold text-foreground">{formatMonthDay(day.date)}</span>
        <span className="tabular-nums text-muted-foreground">{total}</span>
      </div>
      <div className="mb-2 space-y-1">
        {day.by_model.slice(0, 4).map((m) => (
          <Row
            key={m.model}
            label={m.model}
            value={metric === "cost" ? formatCost(m.est_cost_microusd) : formatTokens(m.total_tokens)}
          />
        ))}
      </div>
      <div className="border-t border-border pt-2">
        <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
          Cache split
        </p>
        <div className="space-y-1">
          {KINDS.map((kind) => (
            <Row
              key={kind}
              label={TOKEN_KIND_LABELS[kind]}
              value={formatTokens(kinds[kind])}
              dot={TOKEN_KIND_COLORS[kind]}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function Row({ label, value, dot }: { label: string; value: string; dot?: string }) {
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="flex items-center gap-1.5 truncate text-muted-foreground">
        {dot && <span className="h-2 w-2 shrink-0 rounded-sm" style={{ backgroundColor: dot }} />}
        <span className="truncate">{label}</span>
      </span>
      <span className="shrink-0 tabular-nums text-foreground">{value}</span>
    </div>
  );
}
