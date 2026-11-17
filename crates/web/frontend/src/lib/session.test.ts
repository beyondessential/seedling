import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthRequired, connect, connectBody, forgetSessionToken, sessionDropped } from "./session";

vi.mock("./wt", () => ({
  openWebTransport: vi.fn(async () => ({})),
  WtClient: class {},
}));

afterEach(() => {
  vi.unstubAllGlobals();
  forgetSessionToken();
});

function answer200(token: string) {
  const fetchMock = vi.fn().mockImplementation(
    async () =>
      new Response(
        JSON.stringify({
          token,
          actor: { kind: "password", id: "admin", display: "admin", session: "s" },
          wt_url: "https://localhost:1/wt?t=x",
          cert_hashes: [],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function sentBody(fetchMock: ReturnType<typeof vi.fn>, call: number): unknown {
  return JSON.parse((fetchMock.mock.calls[call][1] as { body: string }).body);
}

function answer401(body: unknown) {
  const fetchMock = vi.fn().mockImplementation(
    async () =>
      new Response(JSON.stringify(body), {
        status: 401,
        headers: { "Content-Type": "application/json" },
      }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

describe("connect", () => {
  // spec: UI#signing-in
  it("reads which credential a 401 asks for", async () => {
    answer401({ auth_required: "password" });
    await expect(connect({})).rejects.toMatchObject({ credential: "password" });
    answer401({ auth_required: "none" });
    const err = await connect({}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AuthRequired);
    expect((err as AuthRequired).credential).toBe("none");
  });
});

describe("connect credentials", () => {
  // spec: WEB#authentication
  it("sends the held session token as token on each later connect", async () => {
    const fetchMock = answer200("t1");
    await connect({ password: "hunter2" });
    await connect({});
    expect(sentBody(fetchMock, 0)).toEqual({ password: "hunter2" });
    expect(sentBody(fetchMock, 1)).toEqual({ token: "t1" });
  });

  // spec: WEB#authentication
  it("sends the newest token a connect issued", async () => {
    answer200("t1");
    await connect({});
    const second = answer200("t2");
    await connect({});
    expect(sentBody(second, 0)).toEqual({ token: "t1" });
    const third = answer200("t3");
    await connect({});
    expect(sentBody(third, 0)).toEqual({ token: "t2" });
  });

  // spec: WEB#authentication
  it("sends at most one of token and password", async () => {
    const fetchMock = answer200("t1");
    await connect({});
    await connect({ password: "hunter2" });
    expect(sentBody(fetchMock, 1)).toEqual({ password: "hunter2" });
    expect(connectBody({ token: "a", password: "b" }, "c")).toEqual({ password: "b" });
    expect(connectBody({}, null)).toEqual({});
  });

  // spec: WEB#authentication
  it("stops sending a token the web interface refused", async () => {
    answer200("t1");
    await connect({});
    const refused = answer401({ auth_required: "password" });
    await expect(connect({})).rejects.toBeInstanceOf(AuthRequired);
    expect(sentBody(refused, 0)).toEqual({ token: "t1" });
    const again = answer200("t3");
    await connect({});
    expect(sentBody(again, 0)).toEqual({});
  });
});

describe("sessionDropped", () => {
  // spec: WEB#session-tracking
  it("is true only for a heartbeat answered alive false", () => {
    expect(sessionDropped({ alive: false, session_id: "s" })).toBe(true);
    expect(sessionDropped({ alive: true, session_id: "s" })).toBe(false);
    expect(sessionDropped({})).toBe(false);
    expect(sessionDropped(null)).toBe(false);
  });
});
