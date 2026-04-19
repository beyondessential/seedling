import { useCallback, useContext, useState } from "react";
import { SessionContext } from "../components/SessionProvider";

export function useOiAction() {
  const { session } = useContext(SessionContext);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const execute = useCallback(
    async (method: string, params: unknown): Promise<unknown> => {
      if (!session) throw new Error("not connected");
      setLoading(true);
      setError(null);
      try {
        const result = await session.client.request(method, params);
        if (!result.ok) {
          throw new Error(`[${result.error.code}] ${result.error.message}`);
        }
        return result.value;
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        throw e;
      } finally {
        setLoading(false);
      }
    },
    [session],
  );

  return { execute, loading, error, clearError: () => setError(null) };
}
