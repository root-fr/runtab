import * as React from "react";
import { AlertTriangle, Inbox, RotateCw } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

interface PanelProps {
  title?: string;
  subtitle?: string;
  action?: React.ReactNode;
  loading: boolean;
  error?: Error;
  empty?: boolean;
  emptyLabel?: string;
  onRetry?: () => void;
  skeleton?: React.ReactNode;
  className?: string;
  bodyClassName?: string;
  children: React.ReactNode;
}

// Single wrapper so every panel gets the same loading -> error -> empty -> data
// state machine and identical framing. Skeletons hold the panel's shape; errors
// stay local (one dead panel never blanks the dashboard).
export function Panel({
  title, subtitle, action, loading, error, empty, emptyLabel = "No data in this range",
  onRetry, skeleton, className, bodyClassName, children,
}: PanelProps) {
  return (
    <Card className={cn("flex flex-col", className)}>
      {(title || action) && (
        <CardHeader className="flex-row items-start justify-between gap-3 space-y-0">
          <div className="space-y-1">
            {title && <CardTitle>{title}</CardTitle>}
            {subtitle && <p className="text-xs text-muted-foreground">{subtitle}</p>}
          </div>
          {action}
        </CardHeader>
      )}
      <CardContent className={cn("flex-1", title ? undefined : "pt-5", bodyClassName)}>
        {loading ? (
          skeleton ?? <Skeleton className="h-40 w-full" />
        ) : error ? (
          <PanelError error={error} onRetry={onRetry} />
        ) : empty ? (
          <PanelEmpty label={emptyLabel} />
        ) : (
          children
        )}
      </CardContent>
    </Card>
  );
}

function PanelError({ error, onRetry }: { error: Error; onRetry?: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 py-10 text-center">
      <AlertTriangle className="h-6 w-6 text-destructive" />
      <div className="space-y-1">
        <p className="text-sm font-medium">Couldn't load this panel</p>
        <p className="max-w-xs text-xs text-muted-foreground">{error.message}</p>
      </div>
      {onRetry && (
        <button
          onClick={onRetry}
          className="inline-flex items-center gap-1.5 rounded-md border border-input px-3 py-1.5 text-xs font-medium transition-colors hover:bg-secondary"
        >
          <RotateCw className="h-3.5 w-3.5" /> Retry
        </button>
      )}
    </div>
  );
}

function PanelEmpty({ label }: { label: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 py-10 text-center text-muted-foreground">
      <Inbox className="h-6 w-6" />
      <p className="text-sm">{label}</p>
    </div>
  );
}
