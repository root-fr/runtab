import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { formatTokens, parseDate } from "@/lib/format";
import { useElementWidth } from "@/lib/useElementWidth";

const HEIGHT = 240;

// One overlaid line per series, keyed off a shared date axis. Callers supply
// each series' pre-projected values so this component stays non-generic — a
// generic default export loses its type parameter through React.lazy.
export interface TimelineSeries {
  label: string;
  stroke: string;
  fill?: string;
  width?: number;
  values: number[];
}

interface TimelineProps {
  dates: string[];
  series: TimelineSeries[];
}

// Lazy-loaded on demand so uPlot and its CSS live in a separate chunk that a
// user who never opens deep history (or the savings trend) never downloads.
// Canvas rendering keeps a year of daily points smooth where SVG would choke.
export default function UplotTimeline({ dates, series }: TimelineProps) {
  const [wrapRef, width] = useElementWidth<HTMLDivElement>();
  const hostRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);

  function buildData(): uPlot.AlignedData {
    const xs = dates.map((d) => parseDate(d).getTime() / 1000);
    return [xs, ...series.map((s) => s.values)];
  }

  useEffect(() => {
    if (!hostRef.current || width === 0) return;
    const opts: uPlot.Options = {
      width,
      height: HEIGHT,
      legend: { show: false },
      cursor: { points: { size: 6 } },
      scales: { x: { time: true } },
      axes: [
        { stroke: "#64748b", grid: { stroke: "rgba(15,23,42,0.07)" }, ticks: { stroke: "rgba(15,23,42,0.07)" } },
        {
          stroke: "#64748b",
          size: 58,
          grid: { stroke: "rgba(15,23,42,0.07)" },
          ticks: { stroke: "rgba(15,23,42,0.07)" },
          values: (_u, vals) => vals.map((v) => formatTokens(v)),
        },
      ],
      series: [
        {},
        ...series.map((s) => ({
          label: s.label,
          stroke: s.stroke,
          fill: s.fill,
          width: s.width ?? 1.5,
        })),
      ],
    };
    const plot = new uPlot(opts, buildData(), hostRef.current);
    plotRef.current = plot;
    return () => {
      plot.destroy();
      plotRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [width]);

  useEffect(() => {
    plotRef.current?.setData(buildData());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dates, series]);

  return (
    <div ref={wrapRef} className="w-full">
      <div className="mb-3 flex items-center gap-4 text-xs text-muted-foreground">
        {series.map((s) => (
          <span key={s.label} className="flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-sm" style={{ backgroundColor: s.stroke }} /> {s.label}
          </span>
        ))}
      </div>
      <div ref={hostRef} />
    </div>
  );
}
