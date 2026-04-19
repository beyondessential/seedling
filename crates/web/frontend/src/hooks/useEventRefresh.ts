import { useCallback, useContext, useEffect, useRef } from "react";
import { SessionContext } from "../components/SessionProvider";
import type { SeedlingEvent } from "../lib/types";

const DEBOUNCE_MS = 500;

/**
 * Calls `refetch` when a relevant event arrives, debounced.
 * If the tab is hidden when the event fires, queues the refresh and
 * runs it when the tab becomes visible again.
 */
export function useEventRefresh(
  refetch: () => void,
  matches: (ev: SeedlingEvent) => boolean,
) {
  const { events } = useContext(SessionContext);
  // Track the previous newest event by identity, not by array length.
  // Tracking length breaks once the buffer is full (capped at 200): length
  // stays constant so new events are never detected.
  const prevFirst = useRef<SeedlingEvent | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dirty = useRef(false);

  const scheduleRefetch = useCallback(() => {
    if (document.hidden) {
      dirty.current = true;
      return;
    }
    // Leading edge: fire immediately on the first event in a quiet window.
    if (timer.current === null) refetch();
    // Trailing edge: also fire after the burst of events settles.
    else clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      refetch();
    }, DEBOUNCE_MS);
  }, [refetch]);

  // When tab becomes visible, run any queued refresh.
  useEffect(() => {
    const onVisible = () => {
      if (!document.hidden && dirty.current) {
        dirty.current = false;
        refetch();
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refetch]);

  // Check for new matching events.
  useEffect(() => {
    if (events.length === 0 || events[0] === prevFirst.current) return;
    const oldFirst = prevFirst.current;
    prevFirst.current = events[0];
    // Find where the previously-newest event now sits to count new arrivals.
    // indexOf returns -1 if it was pushed out of the capped buffer; in that
    // case treat the whole array as new (worst case: a spurious debounced refetch).
    const cutoff = oldFirst ? events.indexOf(oldFirst) : events.length;
    const newCount = cutoff === -1 ? events.length : cutoff;
    for (let i = 0; i < newCount; i++) {
      if (matches(events[i])) {
        scheduleRefetch();
        break;
      }
    }
  }, [events, matches, scheduleRefetch]);

  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);
}
