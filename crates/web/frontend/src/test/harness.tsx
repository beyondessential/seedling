// Test harness: render any route/component against a fake OI session.
//
// Bypasses SessionProvider entirely (no WebTransport, no connect() probe, no
// heartbeat timers) by injecting a hand-built SessionContext value whose
// client answers `request(method, params)` from a fixture map. SafetyMode is
// controlled by seeding sessionStorage before mounting the real
// SafetyModeProvider, since its context is module-local.
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi } from "vitest";
import { SafetyModeProvider } from "../components/SafetyModeProvider";
import { SessionContext } from "../components/SessionProvider";
import type { Session } from "../lib/session";
import type { WtClient } from "../lib/wt";
import type { LogEntry, OiResult, SeedlingEvent } from "../lib/types";

/** A fixture is the raw `value` an OI method resolves with, a full OiResult
 *  (to inject errors), or a function of the request params returning either. */
export type Fixture =
  | unknown
  | ((params: unknown) => unknown);

export interface RenderWithSessionOptions {
  /** Map of OI method path → fixture. Unlisted methods resolve `{ok, value: null}`. */
  fixtures?: Record<string, Fixture>;
  /** Initial URL for the MemoryRouter. */
  route?: string;
  /** Route pattern to mount `ui` under (e.g. "/apps/:name") so useParams works.
   *  When set, `ui` is wrapped in `<Routes><Route path={path} …/></Routes>`. */
  path?: string;
  /** Safety mode active at mount. Defaults to "read". */
  safetyMode?: "read" | "write" | "dangerous";
  /** Events exposed on the session context (consumed by useEventRefresh etc). */
  events?: SeedlingEvent[];
  /** Log entries fed to `streamLogs` consumers, one onEntry call per entry. */
  logEntries?: LogEntry[];
}

function isOiResult(v: unknown): v is OiResult {
  return (
    typeof v === "object" &&
    v !== null &&
    "ok" in v &&
    typeof (v as { ok: unknown }).ok === "boolean" &&
    ((v as { ok: boolean }).ok ? "value" in v : "error" in v)
  );
}

export function makeFakeClient(
  fixtures: Record<string, Fixture> = {},
  logEntries: LogEntry[] = [],
) {
  const request = vi.fn(async (method: string, params: unknown): Promise<OiResult> => {
    if (Object.prototype.hasOwnProperty.call(fixtures, method)) {
      const fixture = fixtures[method];
      const resolved = typeof fixture === "function" ? fixture(params) : fixture;
      return isOiResult(resolved) ? resolved : { ok: true, value: resolved };
    }
    return { ok: true, value: null };
  });

  const streamLogs = vi.fn(
    async (
      _params: unknown,
      onEntry: (entry: LogEntry) => void,
      _signal?: AbortSignal,
    ): Promise<void> => {
      for (const entry of logEntries) onEntry(entry);
    },
  );

  const subscribeEvents = vi.fn(
    (_onEvent: (ev: SeedlingEvent) => void, _signal?: AbortSignal) =>
      new Promise<void>(() => undefined),
  );

  const client = {
    request,
    streamLogs,
    subscribeEvents,
    openShell: vi.fn(),
    openVolumeShell: vi.fn(),
    closed: new Promise<unknown>(() => undefined),
  };
  return { client: client as unknown as WtClient, request, streamLogs };
}

export function renderWithSession(ui: ReactElement, options: RenderWithSessionOptions = {}) {
  const {
    fixtures = {},
    route = "/",
    path,
    safetyMode = "read",
    events = [],
    logEntries = [],
  } = options;

  localStorage.clear();
  sessionStorage.clear();
  if (safetyMode !== "read") {
    sessionStorage.setItem(
      "seedling.safetyMode",
      JSON.stringify({ mode: safetyMode, elevatedUntil: Date.now() + 9 * 60 * 1000 }),
    );
  }

  const { client, request, streamLogs } = makeFakeClient(fixtures, logEntries);
  const session: Session = {
    token: "test-token",
    actor: { kind: "test", id: "test-suite" },
    client,
    wt: { close: () => undefined } as unknown as WebTransport,
  };

  const openShell = vi.fn();
  const openVolumeShell = vi.fn();
  const closeShell = vi.fn();

  const ctx = {
    session,
    probing: false,
    reconnecting: false,
    offline: false,
    setSession: vi.fn(),
    events,
    sidebarOpen: false,
    setSidebarOpen: vi.fn(),
    sidebarWidth: 340,
    setSidebarWidth: vi.fn(),
    uniRouter: null,
    shellTabs: [],
    activeShellId: null,
    setActiveShellId: vi.fn(),
    openShell,
    openVolumeShell,
    closeShell,
    shellsSidebarWidth: 600,
    setShellsSidebarWidth: vi.fn(),
    webSessionId: "test-web-session",
  };

  const inner = path ? (
    <Routes>
      <Route path={path} element={ui} />
    </Routes>
  ) : (
    ui
  );

  const result = render(
    <MemoryRouter initialEntries={[route]}>
      <SafetyModeProvider>
        <SessionContext.Provider value={ctx}>{inner}</SessionContext.Provider>
      </SafetyModeProvider>
    </MemoryRouter>,
  );

  return { ...result, client, request, streamLogs, openShell, openVolumeShell, closeShell };
}
