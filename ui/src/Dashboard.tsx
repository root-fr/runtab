import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import { useAsync } from "@/lib/useAsync";
import { useFilters } from "@/lib/useFilters";
import { StickyBar } from "@/components/layout/StickyBar";
import { RecapStrip } from "@/components/recap/RecapStrip";
import { KpiTiles, type HeroMetric } from "@/components/recap/KpiTiles";
import { HeroSection } from "@/components/sections/HeroSection";
import { ModelsPanel } from "@/components/sections/ModelsPanel";
import { AgentsPanel } from "@/components/sections/AgentsPanel";
import { SavingsPanel } from "@/components/sections/SavingsPanel";
import { ProjectsPanel } from "@/components/sections/ProjectsPanel";
import { PlanPanel } from "@/components/sections/PlanPanel";
import { HeatmapPanel } from "@/components/sections/HeatmapPanel";
import { DeepHistory } from "@/components/charts/DeepHistory";
import { SessionsTable } from "@/components/tables/SessionsTable";
import { Overlays, type OverlayState } from "@/components/sync/Overlays";

const DISMISS_KEY = "runtab.syncBannerDismissed";

export default function Dashboard() {
  const controller = useFilters();
  const { filters } = controller;
  const filterKey = JSON.stringify(filters);

  const summaryQ = useAsync(() => api.summary(filters), [filterKey]);
  const dailyQ = useAsync(() => api.daily(filters), [filterKey]);
  const planQ = useAsync(() => api.planWindow(filters), [filterKey]);
  const syncQ = useAsync(() => api.syncStatus(), []);
  const optionsQ = useAsync(() => api.projects({}), []);
  const agentOptionsQ = useAsync(() => api.agents({}), []);

  const [metric, setMetric] = useState<HeroMetric>("tokens");
  const [grouped, setGrouped] = useState(false);
  const [bannerDismissed, setBannerDismissed] = useState(
    () => window.localStorage.getItem(DISMISS_KEY) === "1",
  );
  const [overlay, setOverlayState] = useState<OverlayState>({
    settings: false, seeWhat: false, review: false, bannerVisible: false,
  });

  // Sync state comes only from the server. No client-side "on" fakery.
  const sync = syncQ.data;

  // Spec: the plan-window gauge, recap, and sync pill refresh every 30s off the
  // background rescan. Re-fetch the live queries (keeping the current filters) so
  // the numbers move, not just the countdown clock.
  useEffect(() => {
    const id = window.setInterval(() => {
      summaryQ.reload();
      planQ.reload();
      syncQ.reload();
    }, 30000);
    return () => window.clearInterval(id);
  }, [summaryQ.reload, planQ.reload, syncQ.reload]);

  const projectOptions = useMemo(
    () => optionsQ.data?.projects.map((p) => p.project_label) ?? [],
    [optionsQ.data],
  );

  const agentOptions = useMemo(
    () => agentOptionsQ.data?.map((a) => a.agent) ?? [],
    [agentOptionsQ.data],
  );

  const setOverlay = useCallback((patch: Partial<OverlayState>) => {
    if (patch.bannerVisible === false) {
      setBannerDismissed(true);
      window.localStorage.setItem(DISMISS_KEY, "1");
    }
    setOverlayState((s) => ({ ...s, ...patch }));
  }, []);

  const onSaved = useCallback(() => {
    summaryQ.reload();
    planQ.reload();
    syncQ.reload();
  }, [summaryQ, planQ, syncQ]);

  const onEnableSync = useCallback(() => setOverlay({ review: true, settings: false }), [setOverlay]);
  const onReviewSaved = useCallback(() => syncQ.reload(), [syncQ.reload]);

  const onSelectProject = useCallback(
    (project: string) => {
      controller.setFilter("project", filters.project === project ? undefined : project);
    },
    [controller, filters.project],
  );

  const onSelectAgent = useCallback(
    (agent: string) => {
      controller.setFilter("agent", filters.agent === agent ? undefined : agent);
    },
    [controller, filters.agent],
  );

  const bannerVisible = !!sync && !sync.enabled && !bannerDismissed;

  return (
    <div className="min-h-screen bg-background text-foreground">
      <StickyBar
        controller={controller}
        projectOptions={projectOptions}
        machines={sync?.machines ?? []}
        agentOptions={agentOptions}
        summary={summaryQ.data}
        planWindow={planQ.data}
        sync={sync}
        onOpenSettings={() => setOverlay({ settings: true })}
      />

      <main className={`mx-auto max-w-7xl space-y-6 px-4 py-6 sm:px-6 ${bannerVisible ? "pb-32" : "pb-16"}`}>
        <RecapStrip summary={summaryQ.data} loading={summaryQ.loading} error={summaryQ.error} />
        <KpiTiles summary={summaryQ.data} loading={summaryQ.loading} active={metric} onSelect={setMetric} />

        <HeroSection state={dailyQ} metric={metric} grouped={grouped} onGroupedChange={setGrouped} />

        <div className="grid gap-6 lg:grid-cols-2">
          <ModelsPanel filters={filters} />
          {agentOptions.length > 1
            ? <AgentsPanel filters={filters} onSelectAgent={onSelectAgent} />
            : <ProjectsPanel filters={filters} onSelectProject={onSelectProject} />}
        </div>

        {agentOptions.length > 1 && (
          <ProjectsPanel filters={filters} onSelectProject={onSelectProject} />
        )}

        <SavingsPanel filters={filters} />

        {summaryQ.data?.plan_window_applicable && <PlanPanel state={planQ} />}

        <HeatmapPanel filters={filters} />
        <DeepHistory days={dailyQ.data?.days ?? []} />
        <SessionsTable filters={filters} />
      </main>

      <Overlays
        sync={sync}
        state={{ ...overlay, bannerVisible }}
        set={setOverlay}
        onSaved={onSaved}
        onEnableSync={onEnableSync}
        onReviewSaved={onReviewSaved}
      />
    </div>
  );
}
