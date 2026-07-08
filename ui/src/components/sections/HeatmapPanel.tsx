import type { Filters } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { Heatmap } from "@/components/charts/Heatmap";

export function HeatmapPanel({ filters }: { filters: Filters }) {
  const filterKey = JSON.stringify(filters);
  const { data, loading, error, reload } = useAsync(() => api.heatmap(filters), [filterKey]);

  return (
    <Panel
      title="52-week activity"
      subtitle="runtab keeps history long after Claude Code prunes its own logs"
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={!!data && data.days.length === 0}
      skeleton={<Skeleton className="h-40 w-full" />}
    >
      {data && <Heatmap data={data} />}
    </Panel>
  );
}
