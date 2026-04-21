import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type SafetyMode = "read" | "write" | "dangerous";
export type SafetyTier = "write" | "dangerous";

const STORAGE_KEY = "seedling.safetyMode";
const RANK: Record<SafetyMode, number> = { read: 0, write: 1, dangerous: 2 };

// 9m59s — starts at "10m" after rounding to minutes and lets the countdown
// step through 9m, 8m… without pausing an extra second on each boundary.
export const ELEVATION_DURATION_MS = 10 * 60 * 1000 - 1_000;

interface StoredState {
  mode: SafetyMode;
  elevatedUntil: number | null;
}

function loadState(): StoredState {
  try {
    const raw = sessionStorage.getItem(STORAGE_KEY);
    if (!raw) return { mode: "read", elevatedUntil: null };
    const parsed = JSON.parse(raw) as Partial<StoredState>;
    if (parsed.mode !== "write" && parsed.mode !== "dangerous") {
      return { mode: "read", elevatedUntil: null };
    }
    const until = typeof parsed.elevatedUntil === "number" ? parsed.elevatedUntil : null;
    if (until === null || until <= Date.now()) {
      return { mode: "read", elevatedUntil: null };
    }
    return { mode: parsed.mode, elevatedUntil: until };
  } catch {
    return { mode: "read", elevatedUntil: null };
  }
}

function storeState(state: StoredState) {
  try {
    if (state.mode === "read") {
      sessionStorage.removeItem(STORAGE_KEY);
    } else {
      sessionStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    }
  } catch {
    // ignore
  }
}

interface SafetyModeCtx {
  mode: SafetyMode;
  setMode: (mode: SafetyMode) => void;
  allowsWrite: boolean;
  allowsDangerous: boolean;
  elevatedUntil: number | null;
}

const SafetyModeContext = createContext<SafetyModeCtx>({
  mode: "read",
  setMode: () => undefined,
  allowsWrite: false,
  allowsDangerous: false,
  elevatedUntil: null,
});

export function SafetyModeProvider({ children }: { children: ReactNode }) {
  const [{ mode, elevatedUntil }, setState] = useState<StoredState>(() => loadState());

  const setMode = useCallback((next: SafetyMode) => {
    const nextState: StoredState =
      next === "read"
        ? { mode: "read", elevatedUntil: null }
        : { mode: next, elevatedUntil: Date.now() + ELEVATION_DURATION_MS };
    setState(nextState);
    storeState(nextState);
  }, []);

  // Auto-revert to read when the elevation window expires. Scheduled via
  // setTimeout so we don't drain the battery with a rerender-per-second
  // countdown in the provider; the switcher renders its own countdown.
  useEffect(() => {
    if (mode === "read" || elevatedUntil === null) return;
    const remaining = elevatedUntil - Date.now();
    if (remaining <= 0) {
      const revert: StoredState = { mode: "read", elevatedUntil: null };
      setState(revert);
      storeState(revert);
      return;
    }
    const t = window.setTimeout(() => {
      const revert: StoredState = { mode: "read", elevatedUntil: null };
      setState(revert);
      storeState(revert);
    }, remaining);
    return () => window.clearTimeout(t);
  }, [mode, elevatedUntil]);

  const value = useMemo<SafetyModeCtx>(
    () => ({
      mode,
      setMode,
      allowsWrite: RANK[mode] >= RANK.write,
      allowsDangerous: RANK[mode] >= RANK.dangerous,
      elevatedUntil,
    }),
    [mode, elevatedUntil, setMode],
  );

  return <SafetyModeContext.Provider value={value}>{children}</SafetyModeContext.Provider>;
}

export function useSafetyMode() {
  return useContext(SafetyModeContext);
}

export interface GuardResult {
  allowed: boolean;
  mode: SafetyMode;
  required: SafetyTier;
  reason: string | null;
}

export function useGuard(required: SafetyTier): GuardResult {
  const { mode } = useSafetyMode();
  const allowed = RANK[mode] >= RANK[required];
  return {
    allowed,
    mode,
    required,
    reason: allowed
      ? null
      : required === "write"
        ? "Read-only mode — switch to Write to enable this action"
        : "Switch to Dangerous mode to enable this destructive action",
  };
}
