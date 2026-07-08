import { lazy, Suspense, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, LineChart } from "lucide-react";
import type { DailyBucket } from "@/api/types";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import type { TimelineSeries } from "./UplotTimeline";

const UplotTimeline = lazy(() => import("./UplotTimeline"));

// Collapsed by default; expanding triggers the dynamic import so the uPlot chunk
// only loads when a user actually wants the deep view.
export function DeepHistory({ days }: { days: DailyBucket[] }) {
  const [open, setOpen] = useState(false);
  const dates = useMemo(() => days.map((d) => d.date), [days]);
  const series = useMemo<TimelineSeries[]>(
    () => [
      { label: "Total tokens", stroke: "#207bff", fill: "rgba(32,123,255,0.12)", width: 1.5, values: days.map((d) => d.total_tokens) },
      {
        label: "Cache read",
        stroke: "#e8a13a",
        width: 1.25,
        values: days.map((d) => d.by_model.reduce((s, m) => s + m.cache_read_tokens, 0)),
      },
    ],
    [days],
  );
  return (
    <Card className="overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-3 p-5 text-left transition-colors hover:bg-secondary/30"
      >
        <LineChart className="h-4 w-4 text-muted-foreground" />
        <div className="flex-1">
          <p className="text-sm font-medium">Deep history timeline</p>
          <p className="text-xs text-muted-foreground">
            Every day since runtab started watching. {days.length} days on record.
          </p>
        </div>
        {open ? (
          <ChevronDown className="h-4 w-4 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-4 w-4 text-muted-foreground" />
        )}
      </button>
      {open && (
        <div className="border-t border-border p-5">
          <Suspense fallback={<Skeleton className="h-60 w-full" />}>
            <UplotTimeline dates={dates} series={series} />
          </Suspense>
        </div>
      )}
    </Card>
  );
}
