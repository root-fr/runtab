import { useEffect, useState } from "react";
import { ShieldCheck } from "lucide-react";
import type { SyncedEvent } from "@/api/types";
import { api } from "@/api/client";
import { Sheet, SheetContent, SheetTrigger } from "@/components/ui/sheet";
import { EXAMPLE_SYNCED_EVENT, PRIVACY_SENTENCE } from "@/lib/privacy";

interface SeeWhatSyncsDrawerProps {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: React.ReactNode;
}

// Renders one real record derived from the user's own ledger (the exact whitelist
// shape and values a push would upload), so the claim "we only send derived
// numbers" is inspectable. Falls back to a labelled sample only when there is no
// local data yet.
export function SeeWhatSyncsDrawer({ open, onOpenChange, trigger }: SeeWhatSyncsDrawerProps) {
  const [record, setRecord] = useState<SyncedEvent>(EXAMPLE_SYNCED_EVENT);
  const [isSample, setIsSample] = useState(true);

  useEffect(() => {
    if (!open) return;
    let active = true;
    api
      .previewRecord()
      .then((r) => {
        if (!active) return;
        setRecord(r.record ?? EXAMPLE_SYNCED_EVENT);
        setIsSample(r.record === null);
      })
      .catch(() => {
        if (!active) return;
        setRecord(EXAMPLE_SYNCED_EVENT);
        setIsSample(true);
      });
    return () => {
      active = false;
    };
  }, [open]);

  const json = JSON.stringify(record, null, 2);
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      {trigger && <SheetTrigger asChild>{trigger}</SheetTrigger>}
      <SheetContent
        title="See exactly what syncs"
        description="This is the entire record. Nothing else is ever uploaded."
        widthClass="max-w-lg"
      >
        <div className="space-y-4">
          <div className="flex items-start gap-2 rounded-md border border-accent/30 bg-accent/10 p-3">
            <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
            <p className="text-sm text-foreground">{PRIVACY_SENTENCE}</p>
          </div>
          <p className="text-xs text-muted-foreground">
            {isSample
              ? "Sample record. No local usage yet, so this is the shape every event takes."
              : "One real record from your ledger. This is exactly what a push would upload."}
          </p>
          <div className="overflow-x-auto rounded-lg border border-border bg-background">
            <pre className="p-4 text-xs leading-relaxed text-foreground">
              <code>{json}</code>
            </pre>
          </div>
          <p className="text-xs text-muted-foreground">
            The server rejects any payload with a field outside this list. The whitelist fails
            closed, so an accidental leak can't be stored even if it were sent.
          </p>
        </div>
      </SheetContent>
    </Sheet>
  );
}
