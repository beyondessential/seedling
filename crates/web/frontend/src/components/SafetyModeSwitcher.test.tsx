import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SafetyModeProvider } from "./SafetyModeProvider";
import { SafetyModeSwitcher, type PeerElevation } from "./SafetyModeSwitcher";

function renderSwitcher(peerElevation?: PeerElevation) {
  return render(
    <SafetyModeProvider>
      <SafetyModeSwitcher peerElevation={peerElevation} />
    </SafetyModeProvider>,
  );
}

function switchTo(label: string) {
  fireEvent.click(screen.getByText("Read-only"));
  fireEvent.click(screen.getByText(label));
}

describe("SafetyModeSwitcher", () => {
  beforeEach(() => {
    sessionStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows the read-only chip by default", () => {
    renderSwitcher();
    expect(screen.getByText("Read-only")).toBeTruthy();
    expect(screen.queryByText(/Write ·/)).toBeNull();
  });

  // w[verify sessions.safety-mode]
  it("switches to write mode with a countdown that ticks down", () => {
    renderSwitcher();
    switchTo("Write");
    expect(screen.getByText("Write · 10m")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(5 * 60_000);
    });
    expect(screen.getByText("Write · 5m")).toBeTruthy();

    // 9m30s elapsed of the 9m59s window: 29s remain, shown in seconds.
    act(() => {
      vi.advanceTimersByTime(4.5 * 60_000);
    });
    expect(screen.getByText("Write · 29s")).toBeTruthy();
  });

  // w[verify sessions.safety-mode]
  it("reverts the chip to read-only when the elevation expires", () => {
    renderSwitcher();
    switchTo("Write");
    act(() => {
      vi.advanceTimersByTime(10 * 60_000);
    });
    expect(screen.getByText("Read-only")).toBeTruthy();
    expect(sessionStorage.getItem("seedling.safetyMode")).toBeNull();
  });

  it("requires confirmation before entering dangerous mode, and cancel keeps read", () => {
    renderSwitcher();
    switchTo("Dangerous");
    expect(screen.getByText("Enable Dangerous mode?")).toBeTruthy();

    fireEvent.click(screen.getByText("Cancel"));
    // Flush the menu/dialog exit transitions so the closed menu's
    // "Read-only" item unmounts and only the chip remains.
    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(screen.getByText("Read-only")).toBeTruthy();
    expect(sessionStorage.getItem("seedling.safetyMode")).toBeNull();
  });

  it("enters dangerous mode after confirming the dialog", () => {
    renderSwitcher();
    switchTo("Dangerous");
    fireEvent.click(screen.getByRole("button", { name: "Enable Dangerous mode" }));
    expect(screen.getByText("Dangerous · 10m")).toBeTruthy();
    const stored = JSON.parse(
      sessionStorage.getItem("seedling.safetyMode") ?? "null",
    );
    expect(stored.mode).toBe("dangerous");
  });

  // w[verify sessions.safety-mode]
  it("flags peer sessions already elevated at each tier in the menu", () => {
    renderSwitcher({ tier: "dangerous", writeCount: 2, dangerousCount: 1 });
    fireEvent.click(screen.getByText("Read-only"));
    expect(
      screen.getByText("2 other operators already at this level"),
    ).toBeTruthy();
    expect(
      screen.getByText("1 other operator already at this level"),
    ).toBeTruthy();
  });
});
