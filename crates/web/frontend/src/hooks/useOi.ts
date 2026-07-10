import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { SessionContext } from "../components/SessionProvider";

export interface OiQueryError {
  method: string;
  message: string;
  stack?: string;
}

export interface OiQueryOptions {
  // When > 0, successful responses are cached in sessionStorage for this many
  // milliseconds, keyed by method + params. A fresh cached entry is returned
  // synchronously without a network call; `refetch()` always bypasses the
  // cache and replaces the stored entry on success.
  cacheMs?: number;
}

export interface OiQueryState<T> {
  data: T | null;
  loading: boolean;
  error: OiQueryError | null;
  refetch: () => void;
  // When the currently displayed `data` was served from the cache, the
  // timestamp (ms since epoch) the entry was stored at. `null` means the data
  // came from a live fetch (or there is no data yet).
  cachedAt: number | null;
}

function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null";
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${stableStringify(obj[k])}`).join(",")}}`;
}

const CACHE_PREFIX = "oiq:";

function cacheKey(method: string, params: unknown): string {
  return `${CACHE_PREFIX}${method}:${stableStringify(params)}`;
}

interface CacheEntry<T> {
  data: T;
  storedAt: number;
  expiresAt: number;
}

function readCache<T>(key: string): CacheEntry<T> | null {
  try {
    const raw = sessionStorage.getItem(key);
    if (!raw) return null;
    const entry = JSON.parse(raw) as CacheEntry<T>;
    if (typeof entry.expiresAt !== "number" || entry.expiresAt < Date.now()) {
      sessionStorage.removeItem(key);
      return null;
    }
    return entry;
  } catch {
    return null;
  }
}

function writeCache<T>(key: string, data: T, ttlMs: number): void {
  try {
    const now = Date.now();
    const entry: CacheEntry<T> = { data, storedAt: now, expiresAt: now + ttlMs };
    sessionStorage.setItem(key, JSON.stringify(entry));
  } catch {
    // sessionStorage can throw on quota exhaustion or in private-mode setups;
    // a cache miss is harmless.
  }
}

/** Drop every cached response for `method`, regardless of params. For views
 *  that learn through an event that a cached listing is stale before its TTL
 *  expires: a later mount refetches live instead of serving the old entry. */
export function invalidateOiQueryCache(method: string): void {
  const prefix = `${CACHE_PREFIX}${method}:`;
  try {
    const doomed: string[] = [];
    for (let i = 0; i < sessionStorage.length; i++) {
      const key = sessionStorage.key(i);
      if (key?.startsWith(prefix)) doomed.push(key);
    }
    for (const key of doomed) sessionStorage.removeItem(key);
  } catch {
    // Same stance as read/write: a broken cache is only a cache miss.
  }
}

export function useOiQuery<T>(
  method: string,
  params: unknown,
  options?: OiQueryOptions,
): OiQueryState<T> {
  const { session } = useContext(SessionContext);
  const cacheMs = options?.cacheMs ?? 0;
  const key = cacheMs > 0 ? cacheKey(method, params) : null;

  // Seed state from cache synchronously so reopening a cached view doesn't
  // flash a spinner.
  const initial = key ? readCache<T>(key) : null;
  const [data, setData] = useState<T | null>(initial?.data ?? null);
  const [cachedAt, setCachedAt] = useState<number | null>(initial?.storedAt ?? null);
  const [loading, setLoading] = useState(initial ? false : Boolean(session));
  const [error, setError] = useState<OiQueryError | null>(null);
  const [force, setForce] = useState(0);

  // Track which `force` tick we've already consumed so that refetch() bypasses
  // the cache exactly once, while param/method changes still check the cache.
  const consumedForceRef = useRef(0);

  const refetch = useCallback(() => setForce((f) => f + 1), []);

  useEffect(() => {
    if (!session) return;

    const bypassCache = force !== consumedForceRef.current;
    consumedForceRef.current = force;

    if (!bypassCache && key) {
      const cached = readCache<T>(key);
      if (cached) {
        setData(cached.data);
        setCachedAt(cached.storedAt);
        setLoading(false);
        setError(null);
        return;
      }
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    session.client
      .request(method, params)
      .then((result) => {
        if (cancelled) return;
        if (result.ok) {
          setData(result.value as T);
          setCachedAt(null);
          if (key) writeCache(key, result.value as T, cacheMs);
        } else {
          setError({
            method,
            message: `[${result.error.code}] ${result.error.message}`,
          });
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          console.error(`[OI] ${method} failed:`, e);
          setError({
            method,
            message: e instanceof Error ? e.message : String(e),
            stack: e instanceof Error ? e.stack : undefined,
          });
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session, method, key, force]);

  return { data, loading, error, refetch, cachedAt };
}
