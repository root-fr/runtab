// Display formatting for tokens (counts), cost (micro-USD), dates, and shares.
// All costs cross the wire as integer micro-USD; divide by 1e6 only here so the
// value the product frames as a dollar figure never drifts through float math.

const TOKEN_UNITS: ReadonlyArray<[number, string]> = [
  [1e9, "B"],
  [1e6, "M"],
  [1e3, "K"],
];

export function formatTokens(n: number): string {
  const abs = Math.abs(n);
  for (const [scale, suffix] of TOKEN_UNITS) {
    if (abs >= scale) {
      const scaled = n / scale;
      const digits = scaled >= 100 ? 0 : 1;
      return `${trimZero(scaled.toFixed(digits))}${suffix}`;
    }
  }
  return String(n);
}

function trimZero(s: string): string {
  return s.replace(/\.0$/, "");
}

export function microUsdToUsd(microusd: number): number {
  return microusd / 1e6;
}

export function formatCost(microusd: number): string {
  const usd = microUsdToUsd(microusd);
  const abs = Math.abs(usd);
  if (abs >= 1000) {
    return `$${formatTokens(usd)}`;
  }
  if (abs >= 10) {
    return `$${usd.toFixed(0)}`;
  }
  return `$${usd.toFixed(2)}`;
}

export function formatPercent(fraction: number, digits = 0): string {
  return `${(fraction * 100).toFixed(digits)}%`;
}

export function formatInt(n: number): string {
  return n.toLocaleString("en-US");
}

const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  opencode: "opencode",
  hermes: "Hermes",
};

export function agentLabel(agent: string): string {
  return AGENT_LABELS[agent] ?? agent;
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

// Parse a YYYY-MM-DD or RFC3339 string without pulling in a date library.
export function parseDate(value: string): Date {
  return new Date(value.length <= 10 ? `${value}T00:00:00Z` : value);
}

export function formatDayLabel(value: string): string {
  const d = parseDate(value);
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}`;
}

export function formatMonthDay(value: string): string {
  const d = parseDate(value);
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}, ${d.getUTCFullYear()}`;
}

export function formatClock(value: string): string {
  const d = parseDate(value);
  const h = String(d.getUTCHours()).padStart(2, "0");
  const m = String(d.getUTCMinutes()).padStart(2, "0");
  return `${h}:${m}`;
}

export function formatDuration(startIso: string, endIso: string): string {
  const ms = parseDate(endIso).getTime() - parseDate(startIso).getTime();
  if (!Number.isFinite(ms) || ms <= 0) return "-";
  const mins = Math.round(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem ? `${hours}h ${rem}m` : `${hours}h`;
}

export function formatCountdown(targetIso: string, nowMs: number): string {
  const ms = parseDate(targetIso).getTime() - nowMs;
  if (!Number.isFinite(ms) || ms <= 0) return "now";
  const totalMinutes = Math.floor(ms / 60000);
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}
