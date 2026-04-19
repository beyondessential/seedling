import { createContext, useState, type ReactNode } from "react";
import type { Session } from "../lib/session";

interface SessionCtx {
  session: Session | null;
  setSession: (s: Session | null) => void;
}

export const SessionContext = createContext<SessionCtx>({
  session: null,
  setSession: () => undefined,
});

export function SessionProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  return (
    <SessionContext.Provider value={{ session, setSession }}>
      {children}
    </SessionContext.Provider>
  );
}
