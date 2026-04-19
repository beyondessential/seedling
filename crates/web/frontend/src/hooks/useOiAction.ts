import { useCallback, useContext, useState } from "react";
import { SessionContext } from "../components/SessionProvider";
import type { OiQueryError } from "./useOi";

export function useOiAction() {
  const { session } = useContext(SessionContext);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<OiQueryError | null>(null);

  const execute = useCallback(
    async (method: string, params: unknown): Promise<unknown> => {
      if (!session) throw new Error("not connected");
      setLoading(true);
      setError(null);
      let errorSet = false;
      try {
        const result = await session.client.request(method, params);
        if (!result.ok) {
          const err: OiQueryError = {
            method,
            message: `[${result.error.code}] ${result.error.message}`,
          };
          setError(err);
          errorSet = true;
          throw new Error(err.message);
        }
        return result.value;
      } catch (e) {
        if (!errorSet) {
          setError({
            method,
            message: e instanceof Error ? e.message : String(e),
            stack: e instanceof Error ? e.stack : undefined,
          });
        }
        throw e;
      } finally {
        setLoading(false);
      }
    },
    [session],
  );

  return { execute, loading, error, clearError: () => setError(null) };
}
