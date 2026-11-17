import { act, render } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as sessionLib from "../lib/session";
import { SafetyModeProvider, useSafetyMode } from "./SafetyModeProvider";
import { SessionProvider } from "./SessionProvider";

vi.mock("../lib/session", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/session")>()),
  connect: vi.fn(),
}));

vi.mock("../lib/uni-router", () => ({
  UniRouter: class {
    startPump() {}
  },
}));

function fakeSession() {
  const request = vi.fn(async () => ({
    ok: true as const,
    value: { alive: true, session_id: "s1" },
  }));
  const close = vi.fn();
  const session = {
    token: "t",
    actor: { kind: "dev", id: "dev" },
    client: {
      request,
      subscribeEvents: vi.fn(() => new Promise<void>(() => undefined)),
      closed: new Promise<unknown>(() => undefined),
    },
    wt: { close },
  };
  return { session: session as unknown as sessionLib.Session, request, close };
}

let setModeFn: (m: "read" | "write" | "dangerous") => void = () => undefined;
function ModeProbe() {
  const { setMode } = useSafetyMode();
  useEffect(() => {
    setModeFn = setMode;
  }, [setMode]);
  return null;
}

async function mount() {
  const fake = fakeSession();
  vi.mocked(sessionLib.connect).mockResolvedValue(fake.session);
  render(
    <SafetyModeProvider>
      <ModeProbe />
      <SessionProvider>
        <div />
      </SessionProvider>
    </SafetyModeProvider>,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  return fake;
}

const heartbeats = (request: ReturnType<typeof vi.fn>) =>
  request.mock.calls.filter((c) => (c as unknown[])[0] === "/connected-clients/heartbeat");

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("SessionProvider heartbeats", () => {
  // spec: WEB#session-tracking
  it("sends a heartbeat with the safety mode as the session opens", async () => {
    const { request } = await mount();
    const calls = heartbeats(request);
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toEqual({ safety_mode: "read" });
  });

  // spec: WEB#session-tracking
  it("sends a heartbeat at once when the safety mode changes, and the mode in every one", async () => {
    const { request } = await mount();
    await act(async () => {
      setModeFn("write");
      await vi.advanceTimersByTimeAsync(0);
    });
    const calls = heartbeats(request);
    expect(calls).toHaveLength(2);
    expect(calls[1][1]).toEqual({ safety_mode: "write" });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(120_000);
    });
    const later = heartbeats(request);
    expect(later.length).toBeGreaterThanOrEqual(3);
    for (const c of later) expect(c[1]).toHaveProperty("safety_mode");
    expect(later[later.length - 1][1]).toEqual({ safety_mode: "write" });
  });

  // spec: WEB#session-tracking
  it("sends a heartbeat at least once every 200 seconds", async () => {
    const { request } = await mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200_000);
    });
    expect(heartbeats(request).length).toBeGreaterThanOrEqual(2);
  });

  // spec: WEB#session-tracking
  it("closes the web session on pagehide without waiting", async () => {
    const { close } = await mount();
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(close).toHaveBeenCalledTimes(1);
  });
});
