import type { HeatmapDay, HeatmapResponse } from "@/api/types";
import { parseDate } from "@/lib/format";

export interface Cell {
  date: string;
  level: 0 | 1 | 2 | 3 | 4;
  tokens: number;
  col: number;
  row: number;
}

export interface Grid {
  cells: Cell[];
  cols: number;
  monthLabels: { col: number; label: string }[];
  horizonCol: number | null;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKS = 53;

function toKey(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function addDays(d: Date, n: number): Date {
  const copy = new Date(d);
  copy.setUTCDate(copy.getUTCDate() + n);
  return copy;
}

// Build a GitHub-style 53-week grid ending at the latest day present. Columns
// are weeks (Sunday-topped); missing days render as level 0. The deletion
// horizon is mapped to its column so the marker line lands on the right week.
export function buildGrid(data: HeatmapResponse): Grid {
  const byDate = new Map(data.days.map((d): [string, HeatmapDay] => [d.date, d]));
  const latestKey = data.days.reduce((m, d) => (d.date > m ? d.date : m), data.days[0]?.date ?? toKey(new Date()));
  const end = parseDate(latestKey);
  let start = addDays(end, -(WEEKS * 7 - 1));
  while (start.getUTCDay() !== 0) start = addDays(start, -1);

  const cells: Cell[] = [];
  const monthLabels: { col: number; label: string }[] = [];
  let lastMonth = -1;
  const totalDays = Math.round((end.getTime() - start.getTime()) / 86400000) + 1;
  for (let i = 0; i < totalDays; i += 1) {
    const d = addDays(start, i);
    const key = toKey(d);
    const col = Math.floor(i / 7);
    const row = d.getUTCDay();
    const hit = byDate.get(key);
    cells.push({ date: key, level: hit?.level ?? 0, tokens: hit?.total_tokens ?? 0, col, row });
    if (row === 0 && d.getUTCMonth() !== lastMonth) {
      monthLabels.push({ col, label: MONTHS[d.getUTCMonth()] });
      lastMonth = d.getUTCMonth();
    }
  }

  let horizonCol: number | null = null;
  if (data.deletion_horizon) {
    const offset = Math.round((parseDate(data.deletion_horizon).getTime() - start.getTime()) / 86400000);
    if (offset >= 0 && offset < totalDays) horizonCol = Math.floor(offset / 7);
  }

  return { cells, cols: Math.ceil(totalDays / 7), monthLabels, horizonCol };
}
