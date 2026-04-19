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
  const prevLength = useRef(events.length);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dirty = useRef(false);

  const scheduleRefetch = useCallback(() => {
    if (document.hidden) {
      dirty.current = true;
      return;
    }
    if (timer.current !== null) clearTimeout(timer.current);
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
    const prev = prevLength.current;
    prevLength.current = events.length;
    if (events.length <= prev) return;
    // New events are prepended, so check events[0..newCount-1].
    const newCount = events.length - prev;
    for (let i = 0; i < newCount; i++) {
      if (matches(events[i])) {
        scheduleRefetch();
        break;
      }
    }
  }, [events, matches, scheduleRefetch]);

  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);
}
