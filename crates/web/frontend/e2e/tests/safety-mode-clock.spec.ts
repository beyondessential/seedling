// Auto-revert behaviour of the safety chip, exercised through Playwright's
// Clock API rather than the wall clock — the elevation window is ~10 min so
// real-time waits are out.
//
// Clock has to be installed before the page boots (the SafetyModeProvider
// captures Date.now() during mount), so each test starts from a fresh page.

import { expect, test } from "../test-fixtures";

const ELEVATION_MS = 10 * 60 * 1000;

test.describe("safety mode auto-revert", () => {
  test("write mode reverts to read after the elevation window", async ({ page, stack }) => {
    await page.clock.install();
    await page.goto(stack.baseUrl);

    await page.getByText("Read-only").first().click();
    await page.getByRole("menuitem", { name: /^Write/ }).click();

    // Now in write mode — chip should reflect that, and the countdown text
    // (e.g. "Write · 10m") should appear within the chip label.
    await expect(page.getByText(/^Write\b/).first()).toBeVisible();

    // Fast-forward past the elevation window. fastForward fires every
    // pending timer in order, so the SafetyModeProvider's setTimeout will
    // run and put us back in read.
    await page.clock.fastForward(ELEVATION_MS + 1_000);

    await expect(page.getByText("Read-only").first()).toBeVisible();
  });

  test("countdown ticks down every second while elevated", async ({ page, stack }) => {
    await page.clock.install();
    await page.goto(stack.baseUrl);

    await page.getByText("Read-only").first().click();
    await page.getByRole("menuitem", { name: /^Write/ }).click();

    // The chip label is "Write · 10m" right at promotion (elevation starts
    // at 9m59s, which formatRemaining rounds up to 10m).
    const chip = page.getByText(/^Write · /).first();
    await expect(chip).toContainText(/10m/);

    // Run for 5 minutes; the chip should now read 5m. We use runFor (not
    // fastForward) so the per-second setInterval fires through, mirroring
    // how a real clock would walk the countdown.
    await page.clock.runFor(5 * 60 * 1000);
    await expect(chip).toContainText(/5m/);
  });
});
