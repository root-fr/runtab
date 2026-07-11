import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import type { Filters } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { agentLabel, formatClock, formatCost, formatDuration, formatInt, formatMonthDay, formatTokens } from "@/lib/format";

const PAGE_SIZE = 25;

export function SessionsTable({ filters }: { filters: Filters }) {
  const [page, setPage] = useState(1);
  const filterKey = JSON.stringify(filters);

  useEffect(() => setPage(1), [filterKey]);

  const { data, loading, error, reload } = useAsync(
    () => api.sessions(filters, page, PAGE_SIZE),
    [filterKey, page],
  );

  const totalPages = data ? Math.max(1, Math.ceil(data.total / data.page_size)) : 1;

  return (
    <Panel
      title="Sessions"
      subtitle={data ? `${formatInt(data.total)} sessions` : undefined}
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={!!data && data.sessions.length === 0}
      emptyLabel="No sessions in this range"
      skeleton={<Skeleton className="h-64 w-full" />}
      action={
        data ? (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <button
              onClick={() => setPage((p) => Math.max(1, p - 1))}
              disabled={page <= 1}
              aria-label="Previous page"
              className="rounded-md border border-input p-1 transition-colors enabled:hover:bg-secondary disabled:opacity-40"
            >
              <ChevronLeft className="h-4 w-4" />
            </button>
            <span className="tabular-nums">
              {page} / {totalPages}
            </span>
            <button
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
              disabled={page >= totalPages}
              aria-label="Next page"
              className="rounded-md border border-input p-1 transition-colors enabled:hover:bg-secondary disabled:opacity-40"
            >
              <ChevronRight className="h-4 w-4" />
            </button>
          </div>
        ) : undefined
      }
    >
      <div className="overflow-x-auto">
        <table className="w-full min-w-[720px] border-collapse text-sm">
          <thead>
            <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted-foreground">
              <th className="py-2 pr-3 font-medium">Project</th>
              <th className="py-2 pr-3 font-medium">Machine</th>
              <th className="py-2 pr-3 font-medium">Agent</th>
              <th className="py-2 pr-3 font-medium">Model</th>
              <th className="py-2 pr-3 font-medium">Started</th>
              <th className="py-2 pr-3 text-right font-medium">Duration</th>
              <th className="py-2 pr-3 text-right font-medium">Events</th>
              <th className="py-2 pr-3 text-right font-medium">Tokens</th>
              <th className="py-2 text-right font-medium">Cost</th>
            </tr>
          </thead>
          <tbody>
            {data?.sessions.map((s) => (
              <tr key={s.session_id} className="border-b border-border/60 transition-colors hover:bg-secondary/30">
                <td className="py-2 pr-3 font-medium text-foreground">{s.project_label}</td>
                <td className="py-2 pr-3 text-muted-foreground">{s.machine_name}</td>
                <td className="py-2 pr-3 text-muted-foreground">{agentLabel(s.agent)}</td>
                <td className="py-2 pr-3 text-muted-foreground">{s.model}</td>
                <td className="py-2 pr-3 tabular-nums text-muted-foreground">
                  {formatMonthDay(s.started_at)} {formatClock(s.started_at)}
                </td>
                <td className="py-2 pr-3 text-right tabular-nums text-muted-foreground">
                  {formatDuration(s.started_at, s.ended_at)}
                </td>
                <td className="py-2 pr-3 text-right tabular-nums text-muted-foreground">{formatInt(s.event_count)}</td>
                <td className="py-2 pr-3 text-right tabular-nums text-foreground">{formatTokens(s.total_tokens)}</td>
                <td className="py-2 text-right tabular-nums font-medium text-foreground">{formatCost(s.est_cost_microusd)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}
