import type { AgentShare } from "@/api/types";
import { agentColor } from "@/lib/colors";
import { agentLabel, formatCost, formatPercent, formatTokens } from "@/lib/format";

interface AgentDonutProps {
  agents: AgentShare[];
  totalTokens: number;
  activeAgent: string | undefined;
  onSelect: (agent: string) => void;
}

const SIZE = 168;
const R = 68;
const STROKE = 22;
const C = 2 * Math.PI * R;

// Clicking a legend row filters the whole page to that agent; clicking the
// active row clears the filter (toggle), mirroring the projects ranking.
export function AgentDonut({ agents, totalTokens, activeAgent, onSelect }: AgentDonutProps) {
  let offset = 0;
  return (
    <div className="flex flex-col items-center gap-5 sm:flex-row sm:items-center">
      <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
        <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} role="img" aria-label="Agent mix">
          <g transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}>
            <circle cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none" stroke="hsl(var(--muted))" strokeWidth={STROKE} />
            {agents.map((a) => {
              const len = a.share * C;
              const dash = <circle
                key={a.agent}
                cx={SIZE / 2}
                cy={SIZE / 2}
                r={R}
                fill="none"
                stroke={agentColor(a.agent)}
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
      <ul className="w-full space-y-1">
        {agents.map((a) => (
          <li key={a.agent}>
            <button
              onClick={() => onSelect(a.agent)}
              aria-pressed={a.agent === activeAgent}
              className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm transition-colors ${
                a.agent === activeAgent ? "ring-1 ring-accent/60" : "hover:bg-secondary/40"
              }`}
            >
              <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ backgroundColor: agentColor(a.agent) }} />
              <span className="truncate text-foreground">{agentLabel(a.agent)}</span>
              <span className="ml-auto shrink-0 tabular-nums text-muted-foreground">
                {formatPercent(a.share)}
              </span>
              <span className="w-14 shrink-0 text-right tabular-nums text-muted-foreground">
                {formatCost(a.est_cost_microusd)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
