import type { DailyResponse } from "@/api/types";
import type { AsyncState } from "@/lib/useAsync";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { HeroBars } from "@/components/charts/HeroBars";
import type { HeroMetric } from "@/components/recap/KpiTiles";

const METRIC_TITLE: Record<HeroMetric, string> = {
  tokens: "Daily tokens by model",
  cost: "Daily value by model",
  burn: "Daily tokens by model",
  cache: "Daily tokens by cache split",
};

interface HeroSectionProps {
  state: AsyncState<DailyResponse>;
  metric: HeroMetric;
  grouped: boolean;
  onGroupedChange: (grouped: boolean) => void;
}

export function HeroSection({ state, metric, grouped, onGroupedChange }: HeroSectionProps) {
  const days = state.data?.days ?? [];
  return (
    <Panel
      title={METRIC_TITLE[metric]}
      subtitle="Hover a day for the model and cache-token breakdown"
      loading={state.loading && !state.data}
      error={state.error}
      onRetry={state.reload}
      empty={!!state.data && days.length === 0}
      skeleton={<Skeleton className="h-[268px] w-full" />}
      action={<GroupedToggle grouped={grouped} onChange={onGroupedChange} />}
    >
      <HeroBars days={days} metric={metric} grouped={grouped} />
    </Panel>
  );
}

function GroupedToggle({ grouped, onChange }: { grouped: boolean; onChange: (g: boolean) => void }) {
  return (
    <div className="flex rounded-md border border-border p-0.5 text-xs">
      {[
        { id: false, label: "Stacked" },
        { id: true, label: "Grouped" },
      ].map((opt) => (
        <button
          key={opt.label}
          onClick={() => onChange(opt.id)}
          aria-pressed={grouped === opt.id}
          className={cn(
            "rounded px-2.5 py-1 font-medium transition-colors",
            grouped === opt.id ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}
