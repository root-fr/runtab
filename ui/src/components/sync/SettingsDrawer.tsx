import { useEffect, useState } from "react";
import type { Settings, SyncStatus } from "@/api/types";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { useAsync } from "@/lib/useAsync";
import { cn } from "@/lib/utils";

interface SettingsDrawerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sync: SyncStatus | undefined;
  onSaved: () => void;
  onEnableSync: () => void;
}

type Override = "auto" | "subscription" | "api";

const MODE_OPTIONS: { id: Override; label: string }[] = [
  { id: "auto", label: "Auto-detect" },
  { id: "subscription", label: "Subscription" },
  { id: "api", label: "Direct API" },
];

function toOverride(value: Settings["billing_mode_override"]): Override {
  return value ?? "auto";
}

export function SettingsDrawer({ open, onOpenChange, sync, onSaved, onEnableSync }: SettingsDrawerProps) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent title="Settings" description="Billing framing, plan limits, and sync.">
        <SettingsBody sync={sync} onSaved={onSaved} onEnableSync={onEnableSync} onClose={() => onOpenChange(false)} />
      </SheetContent>
    </Sheet>
  );
}

function SettingsBody({
  sync, onSaved, onEnableSync, onClose,
}: Pick<SettingsDrawerProps, "sync" | "onSaved" | "onEnableSync"> & { onClose: () => void }) {
  const { data, loading, error } = useAsync(() => api.settings(), []);
  const [form, setForm] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (data) setForm(data);
  }, [data]);

  if (loading || !form) {
    if (error) return <p className="text-sm text-destructive">{error.message}</p>;
    return <Skeleton className="h-72 w-full" />;
  }

  const override = toOverride(form.billing_mode_override);

  async function save(): Promise<void> {
    if (!form) return;
    setSaving(true);
    try {
      await api.updateSettings(form);
      onSaved();
      onClose();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-6">
      <section className="space-y-2">
        <SectionTitle>Sync</SectionTitle>
        <div className="flex items-center justify-between rounded-md border border-border p-3">
          <div className="min-w-0">
            <p className="truncate text-sm text-foreground">
              {sync?.enabled ? sync.account_email ?? "Signed in" : "Sync is off"}
            </p>
            <p className="text-xs text-muted-foreground">
              {sync?.enabled ? `${sync.machines.length} machine(s) linked` : "Back up and merge machines"}
            </p>
          </div>
          <Switch
            checked={!!sync?.enabled}
            ariaLabel="Toggle sync"
            onChange={(v) => {
              if (v) onEnableSync();
            }}
          />
        </div>
      </section>

      <section className="space-y-2">
        <SectionTitle>Billing mode</SectionTitle>
        <p className="text-xs text-muted-foreground">
          We can't always tell how you're billed. If we guessed wrong, override it. This only
          changes how dollars are framed, never the numbers.
        </p>
        <div className="grid grid-cols-3 gap-1 rounded-md border border-border p-1">
          {MODE_OPTIONS.map((opt) => (
            <button
              key={opt.id}
              onClick={() =>
                setForm({ ...form, billing_mode_override: opt.id === "auto" ? null : opt.id })
              }
              className={cn(
                "rounded px-2 py-1.5 text-xs font-medium transition-colors",
                override === opt.id ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-secondary",
              )}
            >
              {opt.label}
            </button>
          ))}
        </div>
      </section>

      <section className="space-y-3">
        <SectionTitle>Plan limits (estimates)</SectionTitle>
        <NumberField
          label="5-hour block"
          value={form.plan_limit_5h_tokens}
          onChange={(v) => setForm({ ...form, plan_limit_5h_tokens: v })}
        />
        <NumberField
          label="Weekly"
          value={form.plan_limit_weekly_tokens}
          onChange={(v) => setForm({ ...form, plan_limit_weekly_tokens: v })}
        />
      </section>

      <section className="space-y-2">
        <SectionTitle>Machine name</SectionTitle>
        <input
          value={form.machine_name}
          onChange={(e) => setForm({ ...form, machine_name: e.target.value })}
          aria-label="Machine name"
          className="h-9 w-full rounded-md border border-input bg-secondary px-3 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
      </section>

      <div className="flex justify-end gap-2 border-t border-border pt-4">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onClick={save} disabled={saving}>
          {saving ? "Saving..." : "Save changes"}
        </Button>
      </div>
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return <h4 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{children}</h4>;
}

function NumberField({ label, value, onChange }: { label: string; value: number; onChange: (v: number) => void }) {
  return (
    <label className="flex items-center justify-between gap-3 text-sm">
      <span className="text-foreground">{label}</span>
      <span className="flex items-center gap-1.5">
        <input
          type="number"
          min={0}
          step={1}
          value={Math.round(value / 1e6)}
          onChange={(e) => onChange(Math.max(0, Number(e.target.value)) * 1e6)}
          className="h-9 w-24 rounded-md border border-input bg-secondary px-2 text-right text-sm tabular-nums text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        <span className="text-xs text-muted-foreground">M tokens</span>
      </span>
    </label>
  );
}
