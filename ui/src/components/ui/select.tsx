import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

export interface SelectOption {
  value: string;
  label: string;
}

interface SelectProps {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  className?: string;
}

// Native select styled to the light theme. Native keeps a11y and keyboard
// behaviour for free and avoids another Radix dependency for a plain picker.
export function Select({ value, options, onChange, ariaLabel, className }: SelectProps) {
  return (
    <div className={cn("relative inline-flex", className)}>
      <select
        aria-label={ariaLabel}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="h-9 w-full appearance-none rounded-md border border-input bg-secondary px-3 pr-8 text-sm text-foreground outline-none transition-colors hover:bg-secondary/70 focus-visible:ring-2 focus-visible:ring-ring"
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value} className="bg-popover text-popover-foreground">
            {opt.label}
          </option>
        ))}
      </select>
      <ChevronDown className="pointer-events-none absolute right-2 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
    </div>
  );
}
