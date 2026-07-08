import type { ModelShare } from "@/api/types";
import { modelColor } from "@/lib/colors";
import { formatCost, formatPercent, formatTokens } from "@/lib/format";

interface ModelDonutProps {
  models: ModelShare[];
  totalTokens: number;
}

const SIZE = 168;
const R = 68;
const STROKE = 22;
const C = 2 * Math.PI * R;

export function ModelDonut({ models, totalTokens }: ModelDonutProps) {
  let offset = 0;
  return (
    <div className="flex flex-col items-center gap-5 sm:flex-row sm:items-center">
      <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
        <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} role="img" aria-label="Model mix">
          <g transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}>
            <circle cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none" stroke="hsl(var(--muted))" strokeWidth={STROKE} />
            {models.map((m) => {
              const len = m.share * C;
              const dash = <circle
                key={m.model}
                cx={SIZE / 2}
                cy={SIZE / 2}
                r={R}
                fill="none"
                stroke={modelColor(m.model)}
                strokeWidth={STROKE}
                strokeDasharray={`${len} ${C - len}`}
                strokeDashoffset={-offset}
              />;
              offset += len;
              return dash;
            })}
          </g>
        </svg>
        <div className="absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-lg font-semibold tabular-nums">{formatTokens(totalTokens)}</span>
          <span className="text-[10px] uppercase tracking-wide text-muted-foreground">tokens</span>
        </div>
      </div>
      <ul className="w-full space-y-2">
        {models.map((m) => (
          <li key={m.model} className="flex items-center gap-2 text-sm">
            <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ backgroundColor: modelColor(m.model) }} />
            <span className="truncate text-foreground">{m.model}</span>
            <span className="ml-auto shrink-0 tabular-nums text-muted-foreground">
              {formatPercent(m.share)}
            </span>
            <span className="w-14 shrink-0 text-right tabular-nums text-muted-foreground">
              {formatCost(m.est_cost_microusd)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
