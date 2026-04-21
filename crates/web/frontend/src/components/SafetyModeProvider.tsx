import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

export type SafetyMode = "read" | "write" | "dangerous";
export type SafetyTier = "write" | "dangerous";

const STORAGE_KEY = "seedling.safetyMode";
const RANK: Record<SafetyMode, number> = { read: 0, write: 1, dangerous: 2 };

function loadMode(): SafetyMode {
  try {
    const v = sessionStorage.getItem(STORAGE_KEY);
    if (v === "write" || v === "dangerous") return v;
  } catch {
    // ignore
  }
  return "read";
}

function storeMode(mode: SafetyMode) {
  try {
    sessionStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // ignore
  }
}

interface SafetyModeCtx {
  mode: SafetyMode;
  setMode: (mode: SafetyMode) => void;
  allowsWrite: boolean;
  allowsDangerous: boolean;
}

const SafetyModeContext = createContext<SafetyModeCtx>({
  mode: "read",
  setMode: () => undefined,
  allowsWrite: false,
  allowsDangerous: false,
});

export function SafetyModeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<SafetyMode>(() => loadMode());

  const setMode = useCallback((next: SafetyMode) => {
    setModeState(next);
    storeMode(next);
  }, []);

  const value = useMemo<SafetyModeCtx>(
    () => ({
      mode,
      setMode,
      allowsWrite: RANK[mode] >= RANK.write,
      allowsDangerous: RANK[mode] >= RANK.dangerous,
    }),
    [mode, setMode],
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
