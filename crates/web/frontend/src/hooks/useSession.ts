import { useCallback, useContext, useState } from "react";
import { SessionContext } from "../components/SessionProvider";
import { AuthRequired, connect } from "../lib/session";

export type SessionState =
  | { status: "unauthenticated" }
  | { status: "connecting" }
  | { status: "authenticated"; actor: import("../lib/types").Actor }
  | { status: "error"; message: string };

export function useSessionState() {
  return useContext(SessionContext);
}

export function useLogin() {
  const { setSession } = useContext(SessionContext);
  const [state, setState] = useState<SessionState>({ status: "unauthenticated" });

  const login = useCallback(
    async (password: string) => {
      setState({ status: "connecting" });
      try {
        const session = await connect({ password });
        setSession(session);
        setState({ status: "authenticated", actor: session.actor });
      } catch (e) {
        if (e instanceof AuthRequired) {
          setState({ status: "error", message: "Invalid password." });
        } else {
          setState({
            status: "error",
            message: e instanceof Error ? e.message : String(e),
          });
        }
      }
    },
    [setSession],
  );

  const reconnect = useCallback(
    async (token: string) => {
      setState({ status: "connecting" });
      try {
        const session = await connect({ token });
        setSession(session);
        setState({ status: "authenticated", actor: session.actor });
        return true;
      } catch {
        setState({ status: "unauthenticated" });
        return false;
      }
    },
    [setSession],
  );

  return { state, login, reconnect };
}
