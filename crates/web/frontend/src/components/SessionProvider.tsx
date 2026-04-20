import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { AuthRequired, connect } from "../lib/session";
import type { Session } from "../lib/session";
import type { SeedlingEvent, VolumeRef } from "../lib/types";
import { UniRouter } from "../lib/uni-router";

const EVENTS_CACHE_SIZE = 200;
const SIDEBAR_STORAGE_KEY = "seedling.eventsSidebar";
const SIDEBAR_WIDTH_STORAGE_KEY = "seedling.eventsSidebarWidth";
const SHELLS_SIDEBAR_WIDTH_STORAGE_KEY = "seedling.shellsSidebarWidth";
const DEFAULT_SIDEBAR_WIDTH = 340;
const DEFAULT_SHELLS_SIDEBAR_WIDTH = 600;

export type ShellTab =
  | { kind: "shell"; id: string; app: string; shellName: string; params: Record<string, string> }
  | { kind: "volume"; id: string; volumes: VolumeRef[]; label: string };

interface SessionCtx {
  session: Session | null;
  probing: boolean;
  reconnecting: boolean;
  setSession: (s: Session | null) => void;
  events: SeedlingEvent[];
  sidebarOpen: boolean;
  setSidebarOpen: (open: boolean) => void;
  sidebarWidth: number;
  setSidebarWidth: (w: number) => void;
  uniRouter: UniRouter | null;
  shellTabs: ShellTab[];
  activeShellId: string | null;
  setActiveShellId: (id: string | null) => void;
  openShell: (app: string, shellName: string, params: Record<string, string>) => void;
  openVolumeShell: (volumes: VolumeRef[], label: string) => void;
  closeShell: (id: string) => void;
  shellsSidebarWidth: number;
  setShellsSidebarWidth: (w: number) => void;
}

export const SessionContext = createContext<SessionCtx>({
  session: null,
  probing: true,
  reconnecting: false,
  setSession: () => undefined,
  events: [],
  sidebarOpen: false,
  setSidebarOpen: () => undefined,
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
  setSidebarWidth: () => undefined,
  uniRouter: null,
  shellTabs: [],
  activeShellId: null,
  setActiveShellId: () => undefined,
  openShell: () => undefined,
  openVolumeShell: () => undefined,
  closeShell: () => undefined,
  shellsSidebarWidth: DEFAULT_SHELLS_SIDEBAR_WIDTH,
  setShellsSidebarWidth: () => undefined,
});

export function useSessionContext() {
  return useContext(SessionContext);
}

let tabIdCounter = 0;

export function SessionProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  const [probing, setProbing] = useState(true);
  const [reconnecting, setReconnecting] = useState(false);
  const [events, setEvents] = useState<SeedlingEvent[]>([]);
  const [uniRouter, setUniRouter] = useState<UniRouter | null>(null);
  const [sidebarOpen, setSidebarOpenState] = useState<boolean>(() => {
    try {
      return localStorage.getItem(SIDEBAR_STORAGE_KEY) === "true";
    } catch {
      return false;
    }
  });
  const [sidebarWidth, setSidebarWidthState] = useState<number>(() => {
    try {
      const v = parseInt(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY) ?? "", 10);
      return isNaN(v) ? DEFAULT_SIDEBAR_WIDTH : Math.max(220, Math.min(v, 800));
    } catch {
      return DEFAULT_SIDEBAR_WIDTH;
    }
  });
  const [shellsSidebarWidth, setShellsSidebarWidthState] = useState<number>(() => {
    try {
      const v = parseInt(localStorage.getItem(SHELLS_SIDEBAR_WIDTH_STORAGE_KEY) ?? "", 10);
      return isNaN(v) ? DEFAULT_SHELLS_SIDEBAR_WIDTH : Math.max(300, Math.min(v, 1200));
    } catch {
      return DEFAULT_SHELLS_SIDEBAR_WIDTH;
    }
  });
  const [shellTabs, setShellTabs] = useState<ShellTab[]>([]);
  const [activeShellId, setActiveShellId] = useState<string | null>(null);
  const probeRan = useRef(false);


  const setSidebarOpen = useCallback((open: boolean) => {
    setSidebarOpenState(open);
    try { localStorage.setItem(SIDEBAR_STORAGE_KEY, String(open)); } catch { /* ignore */ }
  }, []);

  const setSidebarWidth = useCallback((w: number) => {
    setSidebarWidthState(w);
    try { localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(w)); } catch { /* ignore */ }
  }, []);

  const setShellsSidebarWidth = useCallback((w: number) => {
    setShellsSidebarWidthState(w);
    try { localStorage.setItem(SHELLS_SIDEBAR_WIDTH_STORAGE_KEY, String(w)); } catch { /* ignore */ }
  }, []);

  const openShell = useCallback((app: string, shellName: string, params: Record<string, string>) => {
    const id = String(++tabIdCounter);
    const tab: ShellTab = { kind: "shell", id, app, shellName, params };
    setShellTabs((prev) => [...prev, tab]);
    setActiveShellId(id);
  }, []);

  const openVolumeShell = useCallback((volumes: VolumeRef[], label: string) => {
    const id = String(++tabIdCounter);
    const tab: ShellTab = { kind: "volume", id, volumes, label };
    setShellTabs((prev) => [...prev, tab]);
    setActiveShellId(id);
  }, []);

  const closeShell = useCallback((id: string) => {
    setShellTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      setActiveShellId((active) => {
        if (active !== id) return active;
        return next.length > 0 ? next[next.length - 1].id : null;
      });
      return next;
    });
  }, []);

  useEffect(() => {
    if (probeRan.current) return;
    probeRan.current = true;
    connect({})
      .then(setSession)
      .catch((e) => {
        if (!(e instanceof AuthRequired)) {
          console.warn("connect probe failed:", e);
        }
      })
      .finally(() => setProbing(false));
  }, []);

  const doReconnect = useCallback(async (cancelled: { current: boolean }) => {
    setReconnecting(true);
    const deadline = Date.now() + 5 * 60 * 1000;
    let delay = 1000;

    while (Date.now() < deadline) {
      try {
        const newSession = await connect({});
        if (!cancelled.current) {
          setSession(newSession);
          setReconnecting(false);
        }
        return;
      } catch (e) {
        if (e instanceof AuthRequired) {
          if (!cancelled.current) {
            setSession(null);
            setReconnecting(false);
          }
          return;
        }
        await new Promise<void>((r) => setTimeout(r, delay));
        if (cancelled.current) return;
        delay = Math.min(delay * 2, 30_000);
      }
    }

    if (!cancelled.current) {
      setSession(null);
      setReconnecting(false);
    }
  }, []);

  useEffect(() => {
    if (!session) return;
    const cancelled = { current: false };

    session.client.closed
      .then(() => {
        if (!cancelled.current) void doReconnect(cancelled);
      })
      .catch(() => {
        if (!cancelled.current) void doReconnect(cancelled);
      });

    return () => {
      cancelled.current = true;
    };
  }, [session, doReconnect]);

  // Start the uni-stream router pump for the duration of a session.
  // w[shells.wire]
  useEffect(() => {
    if (!session) {
      setUniRouter(null);
      return;
    }
    const router = new UniRouter();
    router.startPump(session.wt);
    setUniRouter(router);
  }, [session]);

  // Subscribe to events for the duration of a session.
  useEffect(() => {
    if (!session) return;
    setEvents([]);
    const abort = new AbortController();

    void session.client
      .subscribeEvents((ev) => {
        setEvents((prev) => {
          const next = [ev, ...prev];
          return next.length > EVENTS_CACHE_SIZE ? next.slice(0, EVENTS_CACHE_SIZE) : next;
        });
      }, abort.signal)
      .catch((e) => {
        if (!abort.signal.aborted) {
          console.warn("events subscription error:", e);
        }
      });

    return () => abort.abort();
  }, [session]);

  return (
    <SessionContext.Provider value={{
      session, probing, reconnecting, setSession,
      events, sidebarOpen, setSidebarOpen, sidebarWidth, setSidebarWidth,
      uniRouter,
      shellTabs, activeShellId, setActiveShellId, openShell, openVolumeShell, closeShell,
      shellsSidebarWidth, setShellsSidebarWidth,
    }}>
      {children}
    </SessionContext.Provider>
  );
}
