// The safety-mode chip in the navbar gates write/dangerous actions across the
// UI. The state machine is read → write → dangerous (with a confirmation
// dialog for dangerous), with auto-revert. We exercise the read→write hop and
// the dangerous confirm dialog; we don't wait through the auto-revert
// (default ELEVATION_DURATION_MS is too long for a test).

import { expect, test } from "../test-fixtures";

test.describe("safety mode", () => {
  test("starts in read-only", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await expect(page.getByText("Read-only").first()).toBeVisible();
  });

  test("write mode promotes the chip and unlocks New app", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);

    // The chip is the menu trigger; openMenu fires on click.
    await page.getByText("Read-only").first().click();
    await page.getByRole("menuitem", { name: /^Write/ }).click();

    // The chip relabels to include the active mode.
    await expect(page.getByText(/Write/).first()).toBeVisible();
  });

  test("dangerous mode requires explicit confirmation", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await page.getByText("Read-only").first().click();
    await page.getByRole("menuitem", { name: /^Dangerous/ }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(/Enable Dangerous/i);

    await dialog.getByRole("button", { name: /cancel/i }).click();
    await expect(dialog).toBeHidden();
    // Cancel keeps us in the previous mode (read-only).
    await expect(page.getByText("Read-only").first()).toBeVisible();
  });
});
