import type { Summary } from "@/api/types";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCost, formatMonthDay, formatTokens } from "@/lib/format";

interface RecapStripProps {
  summary: Summary | undefined;
  loading: boolean;
  error: Error | undefined;
}

// The one-line headline. Copy mirrors the spec's example verbatim in shape:
// "You've used 38.2M tokens ≈ $412 API-equivalent value across 12 projects
// since Jan 3". The cost label is billing-mode aware (value vs spend).
export function RecapStrip({ summary, loading, error }: RecapStripProps) {
  if (loading) {
    return <Skeleton className="h-7 w-full max-w-2xl" />;
  }
  if (error || !summary) {
    return (
      <p className="text-lg text-muted-foreground">
        Your usage recap will appear here once data loads.
      </p>
    );
  }
  if (summary.total_tokens === 0 || !summary.since) {
    return (
      <p className="text-lg text-muted-foreground">
        No usage in this selection yet. runtab fills this in as your agents run.
      </p>
    );
  }
  const projects = `${summary.project_count} ${summary.project_count === 1 ? "project" : "projects"}`;
  return (
    <p className="text-lg leading-relaxed text-foreground sm:text-xl">
      You've used <Em>{formatTokens(summary.total_tokens)} tokens</Em> ≈{" "}
      <Em>{formatCost(summary.est_cost_microusd)}</Em>{" "}
      <span className="text-muted-foreground">{summary.cost_label}</span> across{" "}
      <Em>{projects}</Em> since {formatMonthDay(summary.since)}.
    </p>
  );
}

function Em({ children }: { children: React.ReactNode }) {
  return <span className="font-semibold text-foreground">{children}</span>;
}
