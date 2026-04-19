import { createContext, useEffect, useState, type ReactNode } from "react";
import { AuthRequired, connect } from "../lib/session";
import type { Session } from "../lib/session";

interface SessionCtx {
  session: Session | null;
  probing: boolean;
  setSession: (s: Session | null) => void;
}

export const SessionContext = createContext<SessionCtx>({
  session: null,
  probing: true,
  setSession: () => undefined,
});

export function SessionProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  const [probing, setProbing] = useState(true);

  useEffect(() => {
    connect({})
      .then(setSession)
      .catch((e) => {
        if (!(e instanceof AuthRequired)) {
          console.warn("connect probe failed:", e);
        }
      })
      .finally(() => setProbing(false));
  }, []);

  return (
    <SessionContext.Provider value={{ session, probing, setSession }}>
      {children}
    </SessionContext.Provider>
  );
}
