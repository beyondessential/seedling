// The events sidebar streams the OI event feed to the operator. The trigger
// lives in the navbar ("Show events" tooltip) and toggles a panel that
// renders the cached events list.
//
// w[verify routes.events]
// w[verify sessions.events]

import { expect, test } from "../test-fixtures";

test.describe("events sidebar", () => {
  test("opens via the navbar toggle and shows the Events header", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    const toggle = page.getByRole("button", { name: /show events/i });
    await toggle.click();
    await expect(page.getByRole("heading", { name: /^Events$/ })).toBeVisible();
    // Toggling again hides it; the tooltip flips to "Hide events".
    await page.getByRole("button", { name: /hide events/i }).click();
    await expect(page.getByRole("heading", { name: /^Events$/ })).toBeHidden();
  });

  test("infrastructure section reports proxy + resolver health", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await page.getByRole("button", { name: /show events/i }).click();
    await expect(page.getByRole("heading", { name: /^Infrastructure$/ })).toBeVisible();
    // The proxy/resolver rows are the only stable markers; their text content
    // changes with state so we just assert the headers exist.
    await expect(page.getByText(/^proxy$/i)).toBeVisible();
    await expect(page.getByText(/^resolver$/i)).toBeVisible();
  });
});
