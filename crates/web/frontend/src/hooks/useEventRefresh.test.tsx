// Tests for useEventRefresh: matching events trigger a debounced refetch.
//
// Rendered with a local SessionContext provider (not renderWithSession)
// because the hook reacts to the events array changing across renders, which
// requires rerendering the provider with new values.
import { act, render, screen } from "@testing-library/react";
import { useCallback, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SessionContext } from "../components/SessionProvider";
import type { SeedlingEvent } from "../lib/types";
import { useEventRefresh } from "./useEventRefresh";

function ev(type: string, id: string): SeedlingEvent {
  return { type, timestamp: "2026-07-09T10:00:00Z", id };
}

function Probe({ matches }: { matches: (e: SeedlingEvent) => boolean }) {
  const [count, setCount] = useState(0);
  const refetch = useCallback(() => setCount((c) => c + 1), []);
  useEventRefresh(refetch, matches);
  return <span data-testid="count">{count}</span>;
}

function renderWithEvents(
  matches: (e: SeedlingEvent) => boolean,
  events: SeedlingEvent[] = [],
) {
  const makeCtx = (evs: SeedlingEvent[]) =>
    ({ session: null, events: evs } as unknown as React.ContextType<
      typeof SessionContext
    >);
  const matchesRef = matches;
  const ui = (evs: SeedlingEvent[]) => (
    <SessionContext.Provider value={makeCtx(evs)}>
      <Probe matches={matchesRef} />
    </SessionContext.Provider>
  );
  const result = render(ui(events));
  return {
    setEvents: (evs: SeedlingEvent[]) => result.rerender(ui(evs)),
  };
}

const isFault = (e: SeedlingEvent) => e.type === "FaultFiled";

describe("useEventRefresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("refetches immediately when a matching event arrives", () => {
    const { setEvents } = renderWithEvents(isFault);
    expect(screen.getByTestId("count").textContent).toBe("0");
    setEvents([ev("FaultFiled", "e1")]);
    expect(screen.getByTestId("count").textContent).toBe("1");
  });

  it("ignores non-matching events", () => {
    const { setEvents } = renderWithEvents(isFault);
    setEvents([ev("AppUpdated", "e1")]);
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByTestId("count").textContent).toBe("0");
  });

  it("debounces a burst into a leading and a trailing refetch", () => {
    const { setEvents } = renderWithEvents(isFault);
    setEvents([ev("FaultFiled", "e1")]);
    expect(screen.getByTestId("count").textContent).toBe("1");
    // Two more events inside the debounce window extend it without firing.
    setEvents([ev("FaultFiled", "e2"), ev("FaultFiled", "e1")]);
    act(() => vi.advanceTimersByTime(300));
    setEvents([ev("FaultFiled", "e3"), ev("FaultFiled", "e2"), ev("FaultFiled", "e1")]);
    expect(screen.getByTestId("count").textContent).toBe("1");
    // Trailing edge fires once the burst settles.
    act(() => vi.advanceTimersByTime(500));
    expect(screen.getByTestId("count").textContent).toBe("2");
  });

  it("only inspects events newer than the previously seen newest", () => {
    const { setEvents } = renderWithEvents(isFault);
    const matching = ev("FaultFiled", "e1");
    setEvents([matching]);
    act(() => vi.advanceTimersByTime(2000));
    // Leading + trailing edge of the debounce for the one event.
    expect(screen.getByTestId("count").textContent).toBe("2");
    // A new non-matching event on top does not refetch, even though the old
    // matching event is still sitting in the buffer below it.
    setEvents([ev("AppUpdated", "e2"), matching]);
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByTestId("count").textContent).toBe("2");
  });
});
