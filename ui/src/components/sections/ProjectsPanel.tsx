import type { Filters } from "@/api/types";
import { api } from "@/api/client";
import { Panel } from "@/components/panels/Panel";
import { Skeleton } from "@/components/ui/skeleton";
import { useAsync } from "@/lib/useAsync";
import { ProjectBars } from "@/components/charts/ProjectBars";

interface ProjectsPanelProps {
  filters: Filters;
  onSelectProject: (project: string) => void;
}

export function ProjectsPanel({ filters, onSelectProject }: ProjectsPanelProps) {
  // Rank WITHOUT the project dimension: this panel is the page's project
  // navigation, so with an active project filter it must keep showing the full
  // ranking (active row highlighted) instead of collapsing to a single bar.
  const { project: _ignored, ...rankFilters } = filters;
  const filterKey = JSON.stringify(rankFilters);
  const { data, loading, error, reload } = useAsync(() => api.projects(rankFilters), [filterKey]);

  return (
    <Panel
      title="Projects by value"
      subtitle={
        filters.project
          ? `Filtering: ${filters.project}. Click it again to clear.`
          : "Click a project to filter the page"
      }
      loading={loading && !data}
      error={error}
      onRetry={reload}
      empty={!!data && data.projects.length === 0}
      skeleton={<Skeleton className="h-44 w-full" />}
    >
      {data && (
        <ProjectBars
          projects={data.projects}
          activeProject={filters.project}
          onSelect={onSelectProject}
        />
      )}
    </Panel>
  );
}
