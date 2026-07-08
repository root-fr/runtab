import { useEffect, useState } from "react";
import { ShieldCheck, Terminal } from "lucide-react";
import type { ReviewProject } from "@/api/types";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Sheet, SheetClose, SheetContent } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { PRIVACY_SENTENCE } from "@/lib/privacy";

interface ProjectReviewProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}

interface RowState {
  label: string;
  excluded: boolean;
}

// The consent moment. Labels default to the path basename (never the full path);
// the user renames or excludes before anything is pushed, and the decisions are
// persisted (POST /api/sync/review) so the push path honours them. Enabling sync
// itself is the magic-link flow, so on save we point the user at `runtab sync
// login` rather than faking an "on" state the dashboard cannot actually reach.
export function ProjectReview({ open, onOpenChange, onSaved }: ProjectReviewProps) {
  const [names, setNames] = useState<string[] | null>(null);
  const [rows, setRows] = useState<Record<string, RowState>>({});
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setSaved(false);
    setError(null);
    setNames(null);
    let active = true;
    api
      .review()
      .then((r) => {
        if (!active) return;
        setNames(r.projects.map((p) => p.name));
        const next: Record<string, RowState> = {};
        for (const p of r.projects) next[p.name] = { label: p.label ?? p.name, excluded: p.excluded };
        setRows(next);
      })
      .catch((e: unknown) => active && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      active = false;
    };
  }, [open]);

  const included = (names ?? []).filter((n) => rows[n] && !rows[n].excluded);

  async function confirm(): Promise<void> {
    if (!names) return;
    setSaving(true);
    setError(null);
    try {
      const projects: ReviewProject[] = names.map((n) => {
        const row = rows[n] ?? { label: n, excluded: false };
        const label = row.label.trim();
        return { name: n, label: label && label !== n ? label : null, excluded: row.excluded };
      });
      await api.saveReview(projects);
      onSaved();
      setSaved(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        title="Review before first sync"
        description="Rename or exclude any project. Full paths never leave your machine."
        widthClass="max-w-lg"
      >
        {saved ? (
          <SavedPanel included={included.length} onClose={() => onOpenChange(false)} />
        ) : !names ? (
          <Skeleton className="h-72 w-full" />
        ) : (
          <div className="flex h-full flex-col gap-4">
            <div className="space-y-2 overflow-y-auto pr-1">
              {names.map((p) => {
                const row = rows[p] ?? { label: p, excluded: false };
                return (
                  <div
                    key={p}
                    className="flex items-center gap-3 rounded-md border border-border bg-background/40 p-3"
                  >
                    <input
                      value={row.label}
                      disabled={row.excluded}
                      onChange={(e) => setRows((s) => ({ ...s, [p]: { ...row, label: e.target.value } }))}
                      aria-label={`Label for ${p}`}
                      className="h-8 flex-1 rounded-md border border-input bg-secondary px-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                    />
                    <span className="text-xs text-muted-foreground">
                      {row.excluded ? "Excluded" : "Included"}
                    </span>
                    <Switch
                      checked={!row.excluded}
                      ariaLabel={`Include ${p}`}
                      onChange={(v) => setRows((s) => ({ ...s, [p]: { ...row, excluded: !v } }))}
                    />
                  </div>
                );
              })}
              {names.length === 0 && (
                <p className="text-sm text-muted-foreground">No projects to review yet.</p>
              )}
            </div>
            <div className="mt-auto space-y-3 border-t border-border pt-4">
              {error && <p className="text-xs text-destructive">{error}</p>}
              <div className="flex items-start gap-2 text-xs text-muted-foreground">
                <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
                <span>{PRIVACY_SENTENCE}</span>
              </div>
              <div className="flex items-center justify-end gap-2">
                <SheetClose asChild>
                  <Button variant="ghost" size="sm">
                    Cancel
                  </Button>
                </SheetClose>
                <Button size="sm" onClick={confirm} disabled={saving || included.length === 0}>
                  {saving ? "Saving..." : `Save review for ${included.length} ${included.length === 1 ? "project" : "projects"}`}
                </Button>
              </div>
            </div>
          </div>
        )}
      </SheetContent>
    </Sheet>
  );
}

function SavedPanel({ included, onClose }: { included: number; onClose: () => void }) {
  return (
    <div className="space-y-4">
      <div className="flex items-start gap-2 rounded-md border border-accent/30 bg-accent/10 p-3">
        <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
        <p className="text-sm text-foreground">
          Review saved. {included} {included === 1 ? "project" : "projects"} will sync, and only the
          derived numbers, never prompts, code, or file paths.
        </p>
      </div>
      <div className="space-y-2">
        <p className="text-sm text-foreground">Finish turning on sync from your terminal:</p>
        <div className="flex items-center gap-2 rounded-md border border-border bg-background p-3">
          <Terminal className="h-4 w-4 shrink-0 text-muted-foreground" />
          <code className="text-sm text-foreground">runtab sync login</code>
        </div>
        <p className="text-xs text-muted-foreground">
          Sign-in is a magic link emailed to you. It honours the choices above; your excluded
          projects stay local.
        </p>
      </div>
      <div className="flex justify-end">
        <Button size="sm" onClick={onClose}>
          Done
        </Button>
      </div>
    </div>
  );
}
