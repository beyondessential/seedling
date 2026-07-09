import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ELEVATION_DURATION_MS,
  SafetyModeProvider,
  useGuard,
  useSafetyMode,
} from "./SafetyModeProvider";

const STORAGE_KEY = "seedling.safetyMode";

/** Surfaces the provider's state so tests can assert on it and drive setMode. */
function Probe() {
  const { mode, setMode, allowsWrite, allowsDangerous } = useSafetyMode();
  const writeGuard = useGuard("write");
  const dangerousGuard = useGuard("dangerous");
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <span data-testid="allows-write">{String(allowsWrite)}</span>
      <span data-testid="allows-dangerous">{String(allowsDangerous)}</span>
      <span data-testid="guard-write">{String(writeGuard.allowed)}</span>
      <span data-testid="guard-dangerous">{String(dangerousGuard.allowed)}</span>
      <button onClick={() => setMode("read")}>go read</button>
      <button onClick={() => setMode("write")}>go write</button>
      <button onClick={() => setMode("dangerous")}>go dangerous</button>
    </div>
  );
}

function renderProbe() {
  return render(
    <SafetyModeProvider>
      <Probe />
    </SafetyModeProvider>,
  );
}

function modeShown(): string {
  return screen.getByTestId("mode").textContent ?? "";
}

describe("SafetyModeProvider", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("defaults to read with no stored state", () => {
    renderProbe();
    expect(modeShown()).toBe("read");
    expect(screen.getByTestId("allows-write").textContent).toBe("false");
    expect(screen.getByTestId("allows-dangerous").textContent).toBe("false");
    expect(screen.getByTestId("guard-write").textContent).toBe("false");
  });

  it("restores a stored elevation that has not expired", () => {
    sessionStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ mode: "write", elevatedUntil: Date.now() + 60_000 }),
    );
    renderProbe();
    expect(modeShown()).toBe("write");
    expect(screen.getByTestId("guard-write").textContent).toBe("true");
    expect(screen.getByTestId("guard-dangerous").textContent).toBe("false");
  });

  it("dangerous mode satisfies both write and dangerous guards", () => {
    sessionStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ mode: "dangerous", elevatedUntil: Date.now() + 60_000 }),
    );
    renderProbe();
    expect(modeShown()).toBe("dangerous");
    expect(screen.getByTestId("allows-write").textContent).toBe("true");
    expect(screen.getByTestId("allows-dangerous").textContent).toBe("true");
  });

  it("ignores a stored elevation that has already expired", () => {
    sessionStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ mode: "dangerous", elevatedUntil: Date.now() - 1 }),
    );
    renderProbe();
    expect(modeShown()).toBe("read");
  });

  it("ignores stored state without an expiry timestamp", () => {
    sessionStorage.setItem(STORAGE_KEY, JSON.stringify({ mode: "write" }));
    renderProbe();
    expect(modeShown()).toBe("read");
  });

  it("ignores unparseable stored state", () => {
    sessionStorage.setItem(STORAGE_KEY, "not json {");
    renderProbe();
    expect(modeShown()).toBe("read");
  });

  it("setMode(write) persists the elevation window to sessionStorage", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    renderProbe();
    fireEvent.click(screen.getByText("go write"));
    expect(modeShown()).toBe("write");
    const stored = JSON.parse(sessionStorage.getItem(STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      mode: "write",
      elevatedUntil: 1_000_000 + ELEVATION_DURATION_MS,
    });
  });

  it("setMode(read) clears the stored state", () => {
    renderProbe();
    fireEvent.click(screen.getByText("go dangerous"));
    expect(sessionStorage.getItem(STORAGE_KEY)).not.toBeNull();
    fireEvent.click(screen.getByText("go read"));
    expect(modeShown()).toBe("read");
    expect(sessionStorage.getItem(STORAGE_KEY)).toBeNull();
  });

  // w[verify sessions.safety-mode]
  it("auto-reverts to read when the elevation window expires", () => {
    vi.useFakeTimers();
    renderProbe();
    fireEvent.click(screen.getByText("go write"));
    expect(modeShown()).toBe("write");

    act(() => {
      vi.advanceTimersByTime(ELEVATION_DURATION_MS - 1);
    });
    expect(modeShown()).toBe("write");

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(modeShown()).toBe("read");
    expect(sessionStorage.getItem(STORAGE_KEY)).toBeNull();
  });
});
