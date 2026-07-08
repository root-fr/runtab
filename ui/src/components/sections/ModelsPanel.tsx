import type { Filters } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { ModelDonut } from "@/components/charts/ModelDonut";

export function ModelsPanel({ filters }: { filters: Filters }) {
  const filterKey = JSON.stringify(filters);
  const { data, loading, error, reload } = useAsync(() => api.models(filters), [filterKey]);
  const total = data?.models.reduce((s, m) => s + m.total_tokens, 0) ?? 0;

  return (
    <Panel
      title="Model mix"
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={!!data && data.models.length === 0}
      skeleton={<Skeleton className="h-44 w-full" />}
    >
      {data && <ModelDonut models={data.models} totalTokens={total} />}
    </Panel>
  );
}
