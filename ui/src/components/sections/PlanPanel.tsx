import type { PlanWindowResponse } from "@/api/types";
import type { AsyncState } from "@/lib/useAsync";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { PlanGauge } from "@/components/charts/PlanGauge";

export function PlanPanel({ state }: { state: AsyncState<PlanWindowResponse> }) {
  const { data, loading, error, reload } = state;
  const noWindows = !!data && data.applicable && !data.rolling_5h && !data.weekly;

  return (
    <section id="plan-window" className="scroll-mt-20">
      <Panel
        title="Plan window"
        subtitle="Rolling 5-hour block and weekly usage against your estimated limits"
        loading={loading && !data}
        error={error}
        onRetry={reload}
        empty={noWindows}
        emptyLabel="No plan window applies to this selection"
        skeleton={<Skeleton className="h-44 w-full" />}
      >
        {data && data.applicable && <PlanGauge data={data} />}
      </Panel>
    </section>
  );
}
