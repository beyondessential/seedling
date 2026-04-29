// The Apps landing has an "Active Sessions" section that surfaces the
// connected-clients view. With the test browser navigating the SPA, our
// own WebTransport session populates the list — which doubles as evidence
// that OI requests proxied through WT carry the actor resolved at connect
// time.
//
// We don't try to verify the "Active operators" table here: it's driven
// by daemon-side events that carry an actor, and read-only navigation
// doesn't emit any. That'd need a write action, which we can't run
// without first installing a real app.
//
// w[verify routes.sessions]
// w[verify transport.webtransport]
// w[verify wt.actor]

import { expect, test } from "../test-fixtures";

test("active sessions shows our browser session with the dev actor", async ({ page, stack }) => {
  // /connected-clients/list is fetched on mount and refreshed on session
  // events, but our own session may register *before* the page subscribes
  // to events — leaving the initial 0-row response sticky. The navbar's
  // sessionCount badge is the early-registration tell, so wait for it to
  // pop above zero before asserting on the Apps section. If Apps still
  // hasn't refetched after that, a one-off reload picks up the populated
  // list.
  await page.goto(stack.baseUrl);
  const sessionsLink = page.getByRole("link", { name: /\d+ connected clients?/ });
  await expect(sessionsLink).toBeVisible({ timeout: 15_000 });

  if (!(await page.getByRole("heading", { name: /^Active Sessions$/i }).isVisible())) {
    await page.reload();
  }

  await expect(page.getByRole("heading", { name: /^Active Sessions$/i })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText(/^Web UI \(/)).toBeVisible({ timeout: 15_000 });
  // The "User" cell renders actor_display, falling back to id/kind. Under
  // --dev-no-auth the display name is "dev". Tests in the same worker share
  // a stack, so previous test browsers accumulate as web sessions: scope to
  // the first match rather than asserting uniqueness.
  await expect(page.getByRole("cell", { name: /^dev$/ }).first()).toBeVisible({ timeout: 15_000 });
});
