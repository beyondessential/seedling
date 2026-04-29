// Exercises POST /connect and the WT-token handshake gate. The fixture runs
// seedling-web with --dev-no-auth on a loopback bind, so /connect resolves
// an actor without credentials.
//
// w[verify auth.connect]
// w[verify wt.token]
// w[verify wt.actor]

import { expect, test } from "../test-fixtures";

test.describe("POST /connect", () => {
  test("returns a session token, actor, and a WT URL with an embedded token", async ({
    request,
    stack,
  }) => {
    const res = await request.post(`${stack.baseUrl}/connect`, {
      data: {},
    });
    expect(res.ok()).toBeTruthy();
    const body = (await res.json()) as {
      token: string;
      actor: { kind?: string; id?: string; display?: string; session?: string };
      wt_url: string;
      cert_hashes: string[];
    };
    expect(body.token).toMatch(/.+/);
    expect(body.actor.kind).toBe("dev");
    expect(body.actor.session).toMatch(/.+/);
    // wt_url shape: https://<host>:<port>/wt?t=<token>
    expect(body.wt_url).toMatch(/^https:\/\/[^/]+\/wt\?t=.+/);
    expect(Array.isArray(body.cert_hashes)).toBe(true);
  });

  test("issues a fresh WT token per /connect call", async ({ request, stack }) => {
    const a = await request.post(`${stack.baseUrl}/connect`, { data: {} });
    const b = await request.post(`${stack.baseUrl}/connect`, { data: {} });
    const aBody = (await a.json()) as { wt_url: string };
    const bBody = (await b.json()) as { wt_url: string };
    const tokenA = new URL(aBody.wt_url).searchParams.get("t");
    const tokenB = new URL(bBody.wt_url).searchParams.get("t");
    expect(tokenA).not.toBeNull();
    expect(tokenA).not.toBe(tokenB);
  });
});
