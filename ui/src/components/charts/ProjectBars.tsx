import type { ProjectRow } from "@/api/types";
import { cn } from "@/lib/utils";
import { formatCost, formatPercent, formatTokens } from "@/lib/format";

interface ProjectBarsProps {
  projects: ProjectRow[];
  activeProject: string | undefined;
  onSelect: (project: string) => void;
}

// Horizontal ranking. Clicking a row filters the whole page to that project;
// clicking the active row clears the filter (toggle).
export function ProjectBars({ projects, activeProject, onSelect }: ProjectBarsProps) {
  const max = Math.max(...projects.map((p) => p.share), 0.0001);
  return (
    <ul className="space-y-1.5">
      {projects.map((p) => {
        const isActive = p.project_label === activeProject;
        return (
          <li key={p.project_label}>
            <button
              onClick={() => onSelect(p.project_label)}
              aria-pressed={isActive}
              className={cn(
                "group relative flex w-full items-center gap-3 overflow-hidden rounded-md px-3 py-2 text-left transition-colors duration-150",
                isActive ? "ring-1 ring-accent/60" : "hover:bg-secondary/40",
              )}
            >
              <span
                className={cn(
                  "absolute inset-y-0 left-0 rounded-md transition-colors",
                  isActive ? "bg-accent/25" : "bg-accent/10 group-hover:bg-accent/15",
                )}
                style={{ width: `${(p.share / max) * 100}%` }}
                aria-hidden
              />
              <span className="relative z-10 min-w-0 flex-1 truncate text-sm text-foreground">
                {p.project_label}
              </span>
              <span className="relative z-10 shrink-0 text-xs tabular-nums text-muted-foreground">
                {formatTokens(p.total_tokens)}
              </span>
              <span className="relative z-10 w-16 shrink-0 text-right text-sm font-medium tabular-nums text-foreground">
                {formatCost(p.est_cost_microusd)}
              </span>
              <span className="relative z-10 w-10 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
                {formatPercent(p.share)}
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
