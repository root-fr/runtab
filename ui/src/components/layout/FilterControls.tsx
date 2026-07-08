import type { SyncMachine } from "@/api/types";
import { Select } from "@/components/ui/select";
import type { FiltersController } from "@/lib/useFilters";
import { activePresetId, rangePresets } from "@/lib/dateRanges";

interface FilterControlsProps {
  controller: FiltersController;
  projectOptions: string[];
  machines: SyncMachine[];
}

export function FilterControls({ controller, projectOptions, machines }: FilterControlsProps) {
  const { filters, setFilter, setRange } = controller;
  const presets = rangePresets();
  const activePreset = activePresetId(filters);

  const projectOpts = [
    { value: "", label: "All projects" },
    ...projectOptions.map((p) => ({ value: p, label: p })),
  ];
  const machineOpts = [
    { value: "", label: "All machines" },
    ...machines.map((m) => ({ value: m.machine_id, label: m.machine_name })),
  ];

  function onPreset(id: string): void {
    if (id === "custom") {
      if (!filters.from && !filters.to) {
        setRange(presets.find((p) => p.id === "30")?.from, presets.find((p) => p.id === "30")?.to);
      }
      return;
    }
    const preset = presets.find((p) => p.id === id);
    setRange(preset?.from, preset?.to);
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select
        ariaLabel="Filter by project"
        value={filters.project ?? ""}
        options={projectOpts}
        onChange={(v) => setFilter("project", v || undefined)}
        className="min-w-[9rem]"
      />
      {machines.length > 1 && (
        <Select
          ariaLabel="Filter by machine"
          value={filters.machine ?? ""}
          options={machineOpts}
          onChange={(v) => setFilter("machine", v || undefined)}
          className="min-w-[9rem]"
        />
      )}
      <Select
        ariaLabel="Date range"
        value={activePreset}
        options={presets.map((p) => ({ value: p.id, label: p.label }))}
        onChange={onPreset}
        className="min-w-[9rem]"
      />
      {activePreset === "custom" && (
        <div className="flex items-center gap-1.5">
          <input
            type="date"
            aria-label="From date"
            value={filters.from ?? ""}
            max={filters.to}
            onChange={(e) => setRange(e.target.value || undefined, filters.to)}
            className="h-9 rounded-md border border-input bg-secondary px-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
          <span className="text-xs text-muted-foreground">to</span>
          <input
            type="date"
            aria-label="To date"
            value={filters.to ?? ""}
            min={filters.from}
            onChange={(e) => setRange(filters.from, e.target.value || undefined)}
            className="h-9 rounded-md border border-input bg-secondary px-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </div>
      )}
    </div>
  );
}
