import { useState } from "react";
import type { HeatmapResponse } from "@/api/types";
import { formatMonthDay, formatTokens, parseDate } from "@/lib/format";
import { buildGrid, type Cell } from "./heatmapGrid";

const CELL = 12;
const GAP = 3;
const STEP = CELL + GAP;
const TOP = 20;
const LEFT = 30;
const WEEKDAYS = [["Mon", 1], ["Wed", 3], ["Fri", 5]] as const;
const WEEKDAY = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
// Mon-first order for the "most active by weekday" rollup — reads as a work week.
const WEEKDAY_ORDER = [1, 2, 3, 4, 5, 6, 0];

// Light-theme blue ramp (spec levels 0-4). Level 0 is the empty surface-2 cell.
const LEVEL_FILL = ["#eef2f7", "#cfe2ff", "#93c0ff", "#4b95ff", "#207bff"];
const HORIZON = "#e8a13a";

interface HoverState {
  cell: Cell;
  x: number;
  y: number;
}

export function Heatmap({ data }: { data: HeatmapResponse }) {
  const [hover, setHover] = useState<HoverState | null>(null);
  const grid = buildGrid(data);
  // Which weekdays the user is most active on, summed over the real data days.
  const weekdayTotals = [0, 0, 0, 0, 0, 0, 0];
  for (const d of data.days) weekdayTotals[parseDate(d.date).getUTCDay()] += d.total_tokens;
  const weekdayMax = Math.max(...weekdayTotals, 0);
  const width = LEFT + grid.cols * STEP;
  const height = TOP + 7 * STEP;
  const horizonX = grid.horizonCol === null ? null : LEFT + grid.horizonCol * STEP - GAP / 2;

  return (
    <div className="space-y-3">
      <div className="overflow-x-auto pb-1">
        <div className="relative" style={{ width }}>
          <svg width={width} height={height} role="img" aria-label="52-week usage heatmap">
            {grid.monthLabels.map((m) => (
              <text key={`${m.col}-${m.label}`} x={LEFT + m.col * STEP} y={12} className="fill-muted-foreground text-[10px]">
                {m.label}
              </text>
            ))}
            {WEEKDAYS.map(([label, row]) => (
              <text key={label} x={0} y={TOP + row * STEP + CELL - 2} className="fill-muted-foreground text-[10px]">
                {label}
              </text>
            ))}
            {grid.cells.map((cell) => (
              <rect
                key={cell.date}
                x={LEFT + cell.col * STEP}
                y={TOP + cell.row * STEP}
                width={CELL}
                height={CELL}
                rx={2}
                fill={LEVEL_FILL[cell.level]}
                onMouseEnter={() =>
                  setHover({ cell, x: LEFT + cell.col * STEP, y: TOP + cell.row * STEP + CELL })
                }
                onMouseLeave={() => setHover((h) => (h?.cell.date === cell.date ? null : h))}
              />
            ))}
            {horizonX !== null && (
              <line x1={horizonX} x2={horizonX} y1={TOP - 4} y2={height} stroke={HORIZON} strokeWidth={1.5} strokeDasharray="3 3" />
            )}
          </svg>
          {horizonX !== null && (
            <div
              className="pointer-events-none absolute -top-1 max-w-[220px] -translate-x-1/2 rounded-md border border-amber/40 bg-amber/10 px-2 py-1 text-[10px] leading-tight text-amber"
              style={{ left: Math.min(Math.max(horizonX, 90), width - 90) }}
            >
              Claude Code deleted its logs here. runtab kept going.
            </div>
          )}
          {hover && (
            <div
              className="pointer-events-none absolute z-20 -translate-x-1/2 translate-y-1 whitespace-nowrap rounded-md border border-border bg-popover px-2 py-1 text-xs shadow-lg"
              style={{ left: hover.x + CELL / 2, top: hover.y }}
            >
              <span className="font-medium">{formatTokens(hover.cell.tokens)}</span>{" "}
              <span className="text-muted-foreground">
                tokens · {WEEKDAY[hover.cell.row]}, {formatMonthDay(hover.cell.date)}
              </span>
            </div>
          )}
        </div>
      </div>
      <div className="flex items-center justify-end gap-1.5 text-[10px] text-muted-foreground">
        <span>Less</span>
        {LEVEL_FILL.map((fill, i) => (
          <span key={i} className="h-3 w-3 rounded-sm" style={{ backgroundColor: fill }} />
        ))}
        <span>More</span>
      </div>

      <div className="border-t border-border pt-3">
        <p className="mb-2.5 text-xs text-muted-foreground">Most active by weekday</p>
        <div className="flex flex-col gap-1.5">
          {WEEKDAY_ORDER.map((wd) => {
            const val = weekdayTotals[wd];
            const pct = weekdayMax > 0 ? (val / weekdayMax) * 100 : 0;
            const isMax = weekdayMax > 0 && val === weekdayMax;
            return (
              <div key={wd} className="flex items-center gap-2.5">
                <span className="w-8 shrink-0 text-[0.7rem] text-muted-foreground">{WEEKDAY[wd]}</span>
                <div
                  className="relative h-3 flex-1 overflow-hidden rounded-sm"
                  style={{ backgroundColor: LEVEL_FILL[0] }}
                >
                  <div
                    className="absolute inset-y-0 left-0 rounded-sm transition-[width] duration-500"
                    style={{ width: `${pct}%`, backgroundColor: isMax ? LEVEL_FILL[4] : LEVEL_FILL[2] }}
                  />
                </div>
                <span
                  className={`w-12 shrink-0 text-right text-[0.7rem] tabular-nums ${
                    isMax ? "text-foreground" : "text-muted-foreground"
                  }`}
                >
                  {formatTokens(val)}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
