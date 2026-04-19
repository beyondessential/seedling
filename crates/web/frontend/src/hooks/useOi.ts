import { useCallback, useContext, useEffect, useState } from "react";
import { SessionContext } from "../components/SessionProvider";

export interface OiQueryState<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  refetch: () => void;
}

export function useOiQuery<T>(
  method: string,
  params: unknown,
): OiQueryState<T> {
  const { session } = useContext(SessionContext);
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  const refetch = useCallback(() => setTick((t) => t + 1), []);

  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    session.client
      .request(method, params)
      .then((result) => {
        if (cancelled) return;
        if (result.ok) {
          setData(result.value as T);
        } else {
          setError(`[${result.error.code}] ${result.error.message}`);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled)
          setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session, method, tick]);

  return { data, loading, error, refetch };
}
