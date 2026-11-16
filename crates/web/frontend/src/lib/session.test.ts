import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthRequired, connect, sessionDropped } from "./session";

afterEach(() => vi.unstubAllGlobals());

function answer401(body: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(
      new Response(JSON.stringify(body), {
        status: 401,
        headers: { "Content-Type": "application/json" },
      }),
    ),
  );
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

describe("sessionDropped", () => {
  // spec: WEB#session-tracking
  it("is true only for a heartbeat answered alive false", () => {
    expect(sessionDropped({ alive: false, session_id: "s" })).toBe(true);
    expect(sessionDropped({ alive: true, session_id: "s" })).toBe(false);
    expect(sessionDropped({})).toBe(false);
    expect(sessionDropped(null)).toBe(false);
  });
});
