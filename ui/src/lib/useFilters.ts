import { useCallback, useEffect, useState } from "react";
import type { Filters } from "@/api/types";

const KEYS = ["project", "machine", "agent", "from", "to"] as const;

function readFromUrl(): Filters {
  const params = new URLSearchParams(window.location.search);
  const next: Filters = {};
  for (const key of KEYS) {
    const value = params.get(key);
    if (value) next[key] = value;
  }
  return next;
}

function writeToUrl(filters: Filters): void {
  const params = new URLSearchParams(window.location.search);
  for (const key of KEYS) {
    const value = filters[key];
    if (value) params.set(key, value);
    else params.delete(key);
  }
  const qs = params.toString();
  const url = qs ? `${window.location.pathname}?${qs}` : window.location.pathname;
  window.history.replaceState(null, "", url);
}

export interface FiltersController {
  filters: Filters;
  setFilter: (key: keyof Filters, value: string | undefined) => void;
  setRange: (from: string | undefined, to: string | undefined) => void;
  reset: () => void;
}

// Filter/date state lives in the URL so a view is shareable and survives reload.
export function useFilters(): FiltersController {
  const [filters, setFilters] = useState<Filters>(readFromUrl);

  useEffect(() => {
    const onPop = () => setFilters(readFromUrl());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const commit = useCallback((next: Filters) => {
    setFilters(next);
    writeToUrl(next);
  }, []);

  const setFilter = useCallback(
    (key: keyof Filters, value: string | undefined) => {
      commit({ ...readFromUrl(), [key]: value || undefined });
    },
    [commit],
  );

  const setRange = useCallback(
    (from: string | undefined, to: string | undefined) => {
      commit({ ...readFromUrl(), from: from || undefined, to: to || undefined });
    },
    [commit],
  );

  const reset = useCallback(() => commit({}), [commit]);

  return { filters, setFilter, setRange, reset };
}
