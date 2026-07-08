import { useEffect, useState } from "react";
import { Cloud, X } from "lucide-react";
import { Button } from "@/components/ui/button";

interface SyncBannerProps {
  onSeeWhatSyncs: () => void;
  onEnableSync: () => void;
  onDismiss: () => void;
}

// Non-modal offer that slides in after first paint (never blocks the dashboard)
// and can be dismissed for good. It leads with what leaves the machine, not with
// the ask, so the privacy story is the headline.
export function SyncBanner({ onSeeWhatSyncs, onEnableSync, onDismiss }: SyncBannerProps) {
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const id = window.setTimeout(() => setShown(true), 600);
    return () => window.clearTimeout(id);
  }, []);

  return (
    <div
      className={`fixed inset-x-0 bottom-0 z-30 flex justify-center px-4 pb-4 transition-transform duration-300 ${
        shown ? "translate-y-0" : "translate-y-[120%]"
      }`}
      role="region"
      aria-label="Sync offer"
    >
      <div className="flex w-full max-w-3xl flex-col gap-3 rounded-xl border border-border bg-card/95 p-4 shadow-2xl backdrop-blur sm:flex-row sm:items-center">
        <div className="flex items-start gap-3">
          <span className="mt-0.5 rounded-md bg-accent/15 p-2 text-accent">
            <Cloud className="h-4 w-4" />
          </span>
          <div className="space-y-0.5">
            <p className="text-sm font-medium text-foreground">
              Back up your ledger and merge every machine
            </p>
            <p className="text-xs text-muted-foreground">
              Sync uploads only derived numbers. Never prompts, code, or file paths.
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2 sm:ml-auto">
          <Button variant="ghost" size="sm" onClick={onSeeWhatSyncs}>
            See what syncs
          </Button>
          <Button size="sm" onClick={onEnableSync}>
            Turn on sync
          </Button>
          <button
            onClick={onDismiss}
            aria-label="Dismiss sync offer"
            className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
