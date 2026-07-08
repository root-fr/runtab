import { useEffect, useState } from "react";

// Ticking wall-clock for live countdowns. Default cadence is 30s to match the
// plan-window refresh; pass a faster interval only where a live second matters.
export function useClock(intervalMs = 30000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}
