import { useCallback, useContext, useState } from "react";
import { SessionContext } from "../components/SessionProvider";
import type { OiQueryError } from "./useOi";

export function useOiAction() {
  const { session } = useContext(SessionContext);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<OiQueryError | null>(null);

  /** Runs the OI call and returns its result value, or `null` on any
   *  failure (OI error or transport error), with the failure exposed via
   *  `error`. Never throws — callers must treat `null` as "did not
   *  happen" and skip their success path. OI methods always resolve with
   *  a JSON object, so `null` is unambiguous. */
  const execute = useCallback(
    async (method: string, params: unknown): Promise<unknown | null> => {
      setLoading(true);
      setError(null);
      try {
        if (!session) {
          setError({ method, message: "not connected" });
          return null;
        }
        const result = await session.client.request(method, params);
        if (!result.ok) {
          setError({
            method,
            message: `[${result.error.code}] ${result.error.message}`,
          });
          return null;
        }
        return result.value;
      } catch (e) {
        setError({
          method,
          message: e instanceof Error ? e.message : String(e),
          stack: e instanceof Error ? e.stack : undefined,
        });
        return null;
      } finally {
        setLoading(false);
      }
    },
    [session],
  );

  return { execute, loading, error, clearError: () => setError(null) };
}
