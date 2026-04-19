import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { AuthRequired, connect } from "../lib/session";
import type { Session } from "../lib/session";

interface SessionCtx {
  session: Session | null;
  probing: boolean;
  reconnecting: boolean;
  setSession: (s: Session | null) => void;
}

export const SessionContext = createContext<SessionCtx>({
  session: null,
  probing: true,
  reconnecting: false,
  setSession: () => undefined,
});

export function useSessionContext() {
  return useContext(SessionContext);
}

export function SessionProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  const [probing, setProbing] = useState(true);
  const [reconnecting, setReconnecting] = useState(false);
  const probeRan = useRef(false);

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

  return (
    <SessionContext.Provider value={{ session, probing, reconnecting, setSession }}>
      {children}
    </SessionContext.Provider>
  );
}
