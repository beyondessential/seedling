// Probe-component tests for useOiQuery (cache + error mapping) and useOiAction.
//
// Rendered with a local SessionContext provider rather than renderWithSession
// because these tests need to mount successive probes against the *same*
// sessionStorage contents (the harness clears storage on every render).
import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { SessionContext } from "../components/SessionProvider";
import { makeFakeClient, type Fixture } from "../test/harness";
import type { Session } from "../lib/session";
import { useOiQuery } from "./useOi";
import { useOiAction } from "./useOiAction";

function renderProbe(ui: React.ReactElement, fixtures: Record<string, Fixture> = {}) {
  const { client, request } = makeFakeClient(fixtures);
  const session = {
    token: "test-token",
    actor: { kind: "test", id: "test" },
    client,
    wt: { close: () => undefined },
  } as unknown as Session;
  const ctx = { session, events: [] } as unknown as React.ContextType<
    typeof SessionContext
  >;
  const result = render(
    <SessionContext.Provider value={ctx}>{ui}</SessionContext.Provider>,
  );
  return { ...result, request };
}

function QueryProbe({ cacheMs }: { cacheMs?: number }) {
  const { data, loading, error, refetch, cachedAt } = useOiQuery<unknown>(
    "/probe",
    { scope: "all" },
    cacheMs ? { cacheMs } : undefined,
  );
  return (
    <div>
      <span data-testid="loading">{String(loading)}</span>
      <span data-testid="data">{data === null ? "" : JSON.stringify(data)}</span>
      <span data-testid="error">{error?.message ?? ""}</span>
      <span data-testid="cached">{cachedAt === null ? "live" : "cached"}</span>
      <button onClick={refetch}>refetch</button>
    </div>
  );
}

describe("useOiQuery", () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it("resolves data from a live fetch", async () => {
    const { request } = renderProbe(<QueryProbe />, { "/probe": { n: 1 } });
    expect(screen.getByTestId("loading").textContent).toBe("true");
    expect(await screen.findByText('{"n":1}')).toBeTruthy();
    expect(screen.getByTestId("loading").textContent).toBe("false");
    expect(screen.getByTestId("cached").textContent).toBe("live");
    expect(request).toHaveBeenCalledWith("/probe", { scope: "all" });
  });

  it("maps OI errors to a [code] message string", async () => {
    renderProbe(<QueryProbe />, {
      "/probe": { ok: false, error: { code: "denied", message: "no access" } },
    });
    expect(await screen.findByText("[denied] no access")).toBeTruthy();
    expect(screen.getByTestId("data").textContent).toBe("");
  });

  it("surfaces thrown transport errors as query errors", async () => {
    renderProbe(<QueryProbe />, {
      "/probe": () => {
        throw new Error("stream reset");
      },
    });
    expect(await screen.findByText("stream reset")).toBeTruthy();
  });

  it("caches responses in sessionStorage and serves them without a request", async () => {
    const first = renderProbe(<QueryProbe cacheMs={60_000} />, {
      "/probe": { n: 1 },
    });
    expect(await screen.findByText('{"n":1}')).toBeTruthy();
    const key = 'oiq:/probe:{"scope":"all"}';
    const stored = JSON.parse(sessionStorage.getItem(key)!) as {
      data: unknown;
      storedAt: number;
      expiresAt: number;
    };
    expect(stored.data).toEqual({ n: 1 });
    expect(stored.expiresAt).toBeGreaterThan(Date.now());
    first.unmount();

    // A fresh mount (new client) is served synchronously from the cache.
    const second = renderProbe(<QueryProbe cacheMs={60_000} />, {
      "/probe": { n: 2 },
    });
    expect(screen.getByTestId("data").textContent).toBe('{"n":1}');
    expect(screen.getByTestId("loading").textContent).toBe("false");
    expect(screen.getByTestId("cached").textContent).toBe("cached");
    expect(second.request).not.toHaveBeenCalled();
  });

  it("refetch bypasses the cache and replaces the stored entry", async () => {
    sessionStorage.setItem(
      'oiq:/probe:{"scope":"all"}',
      JSON.stringify({ data: { n: 1 }, storedAt: Date.now(), expiresAt: Date.now() + 60_000 }),
    );
    const { request } = renderProbe(<QueryProbe cacheMs={60_000} />, {
      "/probe": { n: 2 },
    });
    expect(screen.getByTestId("data").textContent).toBe('{"n":1}');
    expect(request).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText("refetch"));
    expect(await screen.findByText('{"n":2}')).toBeTruthy();
    expect(screen.getByTestId("cached").textContent).toBe("live");
    expect(request).toHaveBeenCalledTimes(1);
    const stored = JSON.parse(
      sessionStorage.getItem('oiq:/probe:{"scope":"all"}')!,
    ) as { data: unknown };
    expect(stored.data).toEqual({ n: 2 });
  });

  it("ignores expired cache entries and fetches live", async () => {
    sessionStorage.setItem(
      'oiq:/probe:{"scope":"all"}',
      JSON.stringify({ data: { n: 1 }, storedAt: Date.now() - 120_000, expiresAt: Date.now() - 60_000 }),
    );
    const { request } = renderProbe(<QueryProbe cacheMs={60_000} />, {
      "/probe": { n: 2 },
    });
    expect(await screen.findByText('{"n":2}')).toBeTruthy();
    expect(request).toHaveBeenCalledTimes(1);
  });
});

function ActionProbe() {
  const { execute, loading, error, clearError } = useOiAction();
  const [value, setValue] = useState("");
  return (
    <div>
      <span data-testid="loading">{String(loading)}</span>
      <span data-testid="value">{value}</span>
      <span data-testid="error">{error?.message ?? ""}</span>
      <button
        onClick={() => {
          void execute("/apps/restart", { app: "shop" }).then((v) => {
            if (v !== null) setValue(JSON.stringify(v));
          });
        }}
      >
        go
      </button>
      <button onClick={clearError}>clear</button>
    </div>
  );
}

describe("useOiAction", () => {
  it("executes the request and resolves the value", async () => {
    const { request } = renderProbe(<ActionProbe />, {
      "/apps/restart": { operation_id: "op-1" },
    });
    fireEvent.click(screen.getByText("go"));
    expect(await screen.findByText('{"operation_id":"op-1"}')).toBeTruthy();
    expect(screen.getByTestId("error").textContent).toBe("");
    expect(request).toHaveBeenCalledWith("/apps/restart", { app: "shop" });
  });

  it("maps OI errors, returns null, and clears via clearError", async () => {
    renderProbe(<ActionProbe />, {
      "/apps/restart": { ok: false, error: { code: "busy", message: "try later" } },
    });
    fireEvent.click(screen.getByText("go"));
    expect(await screen.findByText("[busy] try later")).toBeTruthy();
    expect(screen.getByTestId("value").textContent).toBe("");

    fireEvent.click(screen.getByText("clear"));
    expect(screen.getByTestId("error").textContent).toBe("");
  });
});
