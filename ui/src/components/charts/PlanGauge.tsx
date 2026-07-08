import type { PlanWindowResponse, WeeklyWindow } from "@/api/types";
import { formatCountdown, formatTokens } from "@/lib/format";
import { useClock } from "@/lib/useClock";

const W = 176;
const R = 74;
const CX = W / 2;
const CY = 88;

function polar(angle: number): { x: number; y: number } {
  const rad = (angle * Math.PI) / 180;
  return { x: CX + R * Math.cos(rad), y: CY + R * Math.sin(rad) };
}

function arc(startAngle: number, endAngle: number): string {
  const s = polar(startAngle);
  const e = polar(endAngle);
  const large = endAngle - startAngle <= 180 ? 0 : 1;
  return `M ${s.x} ${s.y} A ${R} ${R} 0 ${large} 1 ${e.x} ${e.y}`;
}

function thresholdColor(u: number): string {
  if (u >= 0.9) return "#dc2626";
  if (u >= 0.7) return "#e8a13a";
  return "#2e9e6f";
}

function ArcMeter({ title, window: w, now }: { title: string; window: WeeklyWindow; now: number }) {
  const u = Math.min(Math.max(w.utilization, 0), 1);
  const end = 180 + u * 180;
  const color = thresholdColor(w.utilization);
  return (
    <div className="flex flex-col items-center">
      <p className="mb-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">{title}</p>
      <div className="relative" style={{ width: W, height: CY + 8 }}>
        <svg width={W} height={CY + 8} role="img" aria-label={`${title} utilization`}>
          <path d={arc(180, 360)} fill="none" stroke="hsl(var(--muted))" strokeWidth={12} strokeLinecap="round" />
          {[0.7, 0.9].map((t) => {
            const p = polar(180 + t * 180);
            return <circle key={t} cx={p.x} cy={p.y} r={1.6} fill="hsl(var(--background))" />;
          })}
          <path d={arc(180, end)} fill="none" stroke={color} strokeWidth={12} strokeLinecap="round" />
        </svg>
        <div className="absolute inset-x-0 bottom-1 flex flex-col items-center">
          <span className="text-2xl font-semibold tabular-nums" style={{ color }}>
            {Math.round(w.utilization * 100)}%
          </span>
        </div>
      </div>
      <p className="text-xs tabular-nums text-muted-foreground">
        {formatTokens(w.tokens_used)} / {formatTokens(w.limit_tokens)}
      </p>
      <p className="mt-0.5 text-xs text-muted-foreground">resets in {formatCountdown(w.resets_at, now)}</p>
    </div>
  );
}

export function PlanGauge({ data }: { data: PlanWindowResponse }) {
  const now = useClock(30000);
  return (
    <div className="flex flex-col gap-6">
      <div className="grid grid-cols-1 gap-6 sm:grid-cols-2">
        {data.rolling_5h && <ArcMeter title="5-hour block" window={data.rolling_5h} now={now} />}
        {data.weekly && <ArcMeter title="This week" window={data.weekly} now={now} />}
      </div>
      {data.limits_are_estimates && (
        <p className="text-center text-[11px] text-muted-foreground">
          Limits are reverse-engineered estimates and can drift. Tune them in settings.
        </p>
      )}
    </div>
  );
}
