import type { Filters } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { agentLabel } from "@/lib/format";
import { AgentDonut } from "@/components/charts/AgentDonut";

interface AgentsPanelProps {
  filters: Filters;
  onSelectAgent: (agent: string) => void;
}

export function AgentsPanel({ filters, onSelectAgent }: AgentsPanelProps) {
  // Rank WITHOUT the agent dimension so an active agent filter keeps the full
  // ranking visible (active row highlighted) instead of collapsing to one slice.
  const { agent: _ignored, ...rankFilters } = filters;
  const filterKey = JSON.stringify(rankFilters);
  const { data, loading, error, reload } = useAsync(() => api.agents(rankFilters), [filterKey]);
  const total = data?.reduce((s, a) => s + a.total_tokens, 0) ?? 0;

  return (
    <Panel
      title="Agent mix"
      subtitle={
        filters.agent
          ? `Filtering: ${agentLabel(filters.agent)}. Click it again to clear.`
          : "Click an agent to filter the page"
      }
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={!!data && data.length === 0}
      skeleton={<Skeleton className="h-44 w-full" />}
    >
      {data && (
        <AgentDonut
          agents={data}
          totalTokens={total}
          activeAgent={filters.agent}
          onSelect={onSelectAgent}
        />
      )}
    </Panel>
  );
}
