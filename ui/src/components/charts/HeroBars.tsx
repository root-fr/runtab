import { useState } from "react";
import type { DailyBucket } from "@/api/types";
import { formatCost, formatDayLabel, formatTokens } from "@/lib/format";
import { useElementWidth } from "@/lib/useElementWidth";
import type { HeroMetric } from "@/components/recap/KpiTiles";
import { collectModels, daySegments, heroLegend, type Segment } from "./heroData";
import { HeroTooltip } from "./HeroTooltip";

interface HeroBarsProps {
  days: DailyBucket[];
  metric: HeroMetric;
  grouped: boolean;
}

const H = 268;
const M = { top: 12, bottom: 24, left: 48, right: 10 };

function niceMax(value: number): number {
  if (value <= 0) return 1;
  const pow = Math.pow(10, Math.floor(Math.log10(value)));
  const norm = value / pow;
  const step = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
  return step * pow;
}

function fmt(metric: HeroMetric, v: number): string {
  return metric === "cost" ? formatCost(v) : formatTokens(v);
}

export function HeroBars({ days, metric, grouped }: HeroBarsProps) {
  const [ref, width] = useElementWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);

  const modelOrder = collectModels(days);
  const columns = days.map((day) => daySegments(day, metric, modelOrder));
  const legend = heroLegend(metric, modelOrder);

  const stackedMax = Math.max(...columns.map((c) => c.reduce((s, seg) => s + seg.value, 0)), 1);
  const groupedMax = Math.max(...columns.flatMap((c) => c.map((seg) => seg.value)), 1);
  const yMax = niceMax(grouped ? groupedMax : stackedMax);

  const plotW = Math.max(width - M.left - M.right, 0);
  const plotH = H - M.top - M.bottom;
  const band = days.length ? plotW / days.length : 0;
  const barW = Math.min(band * 0.7, 34);
  const yFor = (v: number) => M.top + plotH - (v / yMax) * plotH;
  const ticks = [0, 0.25, 0.5, 0.75, 1].map((t) => t * yMax);
  const labelEvery = Math.ceil(days.length / Math.max(Math.floor(plotW / 64), 1));

  return (
    <div ref={ref} className="relative w-full select-none">
      {width > 0 && (
        <svg width={width} height={H} role="img" aria-label="Daily usage by model">
          {ticks.map((t) => (
            <g key={t}>
              <line x1={M.left} x2={width - M.right} y1={yFor(t)} y2={yFor(t)} stroke="hsl(var(--border))" strokeWidth={1} />
              <text x={M.left - 8} y={yFor(t) + 3} textAnchor="end" className="fill-muted-foreground text-[10px] tabular-nums">
                {fmt(metric, t)}
              </text>
            </g>
          ))}
          {days.map((day, i) => {
            const cx = M.left + band * i + band / 2;
            return (
              <g key={day.date}>
                {grouped
                  ? renderGrouped(columns[i], cx, barW, yFor, yMax, plotH)
                  : renderStacked(columns[i], cx, barW, yFor)}
                {i % labelEvery === 0 && (
                  <text x={cx} y={H - 8} textAnchor="middle" className="fill-muted-foreground text-[10px]">
                    {formatDayLabel(day.date)}
                  </text>
                )}
                {hover === i && (
                  <rect
                    x={M.left + band * i}
                    y={M.top}
                    width={band}
                    height={plotH}
                    fill="hsl(var(--foreground) / 0.04)"
                    pointerEvents="none"
                  />
                )}
                {/* Hover capture stays last (topmost): if the highlight rendered
                    above it, its mount would steal the pointer and fire
                    mouseleave, unmounting itself, an enter/leave flicker loop. */}
                <rect
                  x={M.left + band * i}
                  y={M.top}
                  width={band}
                  height={plotH}
                  fill="transparent"
                  onMouseEnter={() => setHover(i)}
                  onMouseLeave={() => setHover((h) => (h === i ? null : h))}
                />
              </g>
            );
          })}
        </svg>
      )}
      {hover !== null && days[hover] && (
        <HeroTooltip
          day={days[hover]}
          metric={metric}
          left={Math.min(M.left + band * hover + band, width)}
          flip={M.left + band * hover + band + 224 > width}
        />
      )}
      <Legend items={legend} />
    </div>
  );
}

function renderStacked(segments: Segment[], cx: number, barW: number, yFor: (v: number) => number) {
  let acc = 0;
  return segments.map((seg) => {
    if (seg.value <= 0) return null;
    acc += seg.value;
    const top = yFor(acc);
    const bottom = yFor(acc - seg.value);
    return (
      <rect key={seg.key} x={cx - barW / 2} y={top} width={barW} height={Math.max(bottom - top, 0.5)} fill={seg.color} rx={1} />
    );
  });
}

function renderGrouped(
  segments: Segment[], cx: number, barW: number, yFor: (v: number) => number, yMax: number, plotH: number,
) {
  const active = segments.filter((s) => s.value > 0);
  const sub = active.length ? barW / active.length : barW;
  const start = cx - barW / 2;
  return active.map((seg, j) => {
    const h = (seg.value / yMax) * plotH;
    return (
      <rect key={seg.key} x={start + sub * j} y={yFor(seg.value)} width={Math.max(sub - 1, 1)} height={Math.max(h, 0.5)} fill={seg.color} rx={1} />
    );
  });
}

function Legend({ items }: { items: { key: string; label: string; color: string }[] }) {
  return (
    <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1">
      {items.map((item) => (
        <span key={item.key} className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="h-2.5 w-2.5 rounded-sm" style={{ backgroundColor: item.color }} />
          {item.label}
        </span>
      ))}
    </div>
  );
}
