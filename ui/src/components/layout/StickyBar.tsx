import { Cloud, CloudOff, Gauge, Laptop } from "lucide-react";
import type { PlanWindowResponse, Summary, SyncMachine, SyncStatus } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import type { FiltersController } from "@/lib/useFilters";
import { FilterControls } from "./FilterControls";

interface StickyBarProps {
  controller: FiltersController;
  projectOptions: string[];
  machines: SyncMachine[];
  summary: Summary | undefined;
  planWindow: PlanWindowResponse | undefined;
  sync: SyncStatus | undefined;
  onOpenSettings: () => void;
}

function planTone(u: number): "ok" | "warn" | "danger" {
  if (u >= 0.9) return "danger";
  if (u >= 0.7) return "warn";
  return "ok";
}

function syncTone(state: SyncStatus["state"]): "ok" | "warn" | "muted" {
  if (state === "ok") return "ok";
  if (state === "degraded") return "warn";
  return "muted";
}

const SYNC_LABEL: Record<SyncStatus["state"], string> = {
  ok: "Synced",
  degraded: "Sync degraded",
  off: "Sync off",
};

export function StickyBar({
  controller, projectOptions, machines, summary, planWindow, sync, onOpenSettings,
}: StickyBarProps) {
  const showPlan = summary?.plan_window_applicable && planWindow?.rolling_5h;
  return (
    <header className="sticky top-0 z-30 border-b border-border bg-background/85 backdrop-blur">
      <div className="mx-auto flex max-w-7xl flex-wrap items-center gap-3 px-4 py-3 sm:px-6">
        <span className="text-sm font-semibold tracking-tight">runtab</span>
        <FilterControls controller={controller} projectOptions={projectOptions} machines={machines} />
        <div className="ml-auto flex items-center gap-2">
          {showPlan && planWindow?.rolling_5h && (
            <a href="#plan-window" aria-label="Jump to plan window">
              <Badge tone={planTone(planWindow.rolling_5h.utilization)}>
                <Gauge className="h-3.5 w-3.5" />
                5h {Math.round(planWindow.rolling_5h.utilization * 100)}%
              </Badge>
            </a>
          )}
          <button onClick={onOpenSettings} aria-label="Open sync settings">
            <Badge tone={syncTone(sync?.state ?? "off")}>
              {sync?.state === "off" ? <CloudOff className="h-3.5 w-3.5" /> : <Cloud className="h-3.5 w-3.5" />}
              {SYNC_LABEL[sync?.state ?? "off"]}
              {machines.length > 1 && (
                <span className="flex items-center gap-0.5">
                  <Laptop className="h-3.5 w-3.5" />
                  {machines.length}
                </span>
              )}
            </Badge>
          </button>
        </div>
      </div>
    </header>
  );
}
