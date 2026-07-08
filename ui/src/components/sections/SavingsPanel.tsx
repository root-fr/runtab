import { lazy, Suspense, useMemo } from "react";
import type { Filters, SavingsWindow } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { formatPercent, formatTokens } from "@/lib/format";
import type { TimelineSeries } from "@/components/charts/UplotTimeline";

const UplotTimeline = lazy(() => import("@/components/charts/UplotTimeline"));

const CONSUMED_COLOR = "#207bff";
const SAVED_COLOR = "#2e9e6f";

const CAVEAT = "Savings are rtk's own pre/post-filter estimate, on shell-output tokens.";

export function SavingsPanel({ filters }: { filters: Filters }) {
  const filterKey = JSON.stringify(filters);
  const { data, loading, error, reload } = useAsync(() => api.savings(filters), [filterKey]);

  const dates = useMemo(() => data?.daily.map((d) => d.date) ?? [], [data]);
  const series = useMemo<TimelineSeries[]>(
    () => [
      { label: "Consumed", stroke: CONSUMED_COLOR, fill: "rgba(32,123,255,0.12)", width: 1.5, values: data?.daily.map((d) => d.consumed) ?? [] },
      { label: "Saved by rtk", stroke: SAVED_COLOR, fill: "rgba(46,158,111,0.14)", width: 1.5, values: data?.daily.map((d) => d.saved) ?? [] },
    ],
    [data],
  );

  // rtk never imported: the panel is not part of this deployment's story at all.
  if (data && !data.rtk_present) return null;

  const win = data?.window;
  const noAttributed = !!win && win.saved_attributed === 0;

  return (
    <Panel
      title="rtk savings vs. real consumption"
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={noAttributed}
      emptyLabel="No attributed savings in this view"
      skeleton={<Skeleton className="h-60 w-full" />}
    >
      {data && win && (
        <div className="space-y-5">
          <Headline win={win} />

          <Suspense fallback={<Skeleton className="h-60 w-full" />}>
            <UplotTimeline dates={dates} series={series} />
          </Suspense>

          {win.saved_unattributed != null && (
            <div className="flex flex-wrap items-center gap-x-6 gap-y-1 text-xs text-muted-foreground">
              <span>
                Attributed{" "}
                <span className="font-medium text-foreground tabular-nums">{formatTokens(win.saved_attributed)}</span>
              </span>
              <span>
                Unattributed{" "}
                <span className="font-medium text-foreground tabular-nums">{formatTokens(win.saved_unattributed)}</span>{" "}
                (grep-noise that never reached a model)
              </span>
            </div>
          )}

          <p className="text-xs text-muted-foreground">{CAVEAT}</p>
        </div>
      )}
    </Panel>
  );
}

function Headline({ win }: { win: SavingsWindow }) {
  const ratio = win.savings_ratio == null ? "—" : formatPercent(win.savings_ratio, 1);
  return (
    <p className="text-lg text-muted-foreground">
      rtk avoided{" "}
      <span className="font-semibold text-foreground tabular-nums">~{formatTokens(win.saved_attributed)}</span>{" "}
      tokens against{" "}
      <span className="font-semibold text-foreground tabular-nums">{formatTokens(win.consumed_tokens)}</span>{" "}
      you actually spent —{" "}
      <span className="font-semibold text-foreground tabular-nums">{ratio}</span> of your real consumption.
    </p>
  );
}
