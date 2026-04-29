// Apps landing + create-app flow up to the validation gate. We don't try to
// create a real app (script preview wants a non-trivial Rhai snippet that
// the stub backends would happily accept but that we'd then have to keep
// in sync with the language).

import { expect, test } from "../test-fixtures";

async function enableWriteMode(page: import("@playwright/test").Page) {
  await page.getByText("Read-only").first().click();
  await page.getByRole("menuitem", { name: /^Write/ }).click();
}

test.describe("apps", () => {
  test("empty list shows the placeholder row", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await expect(page.getByText("No apps registered.")).toBeVisible();
  });

  test("New app is disabled in read-only safety mode", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await expect(page.getByRole("button", { name: /new app/i })).toBeDisabled();
  });

  test("New app navigates to /apps/new once write mode is enabled", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await enableWriteMode(page);
    await page.getByRole("button", { name: /new app/i }).click();
    await expect(page).toHaveURL(/\/apps\/new$/);
    await expect(page.getByLabel("App name")).toBeVisible();
  });

  test("invalid app name surfaces the validation message", async ({ page, stack }) => {
    await page.goto(`${stack.baseUrl}/apps/new`);
    const name = page.getByLabel("App name");
    await name.fill("ab"); // too short
    await name.blur();
    await expect(page.getByText(/at least 3 characters/i)).toBeVisible();
  });

  test("Review & create stays disabled while the script editor is empty", async ({ page, stack }) => {
    await page.goto(`${stack.baseUrl}/apps/new`);
    await enableWriteMode(page);
    const review = page.getByRole("button", { name: /review & create/i });
    await expect(review).toBeDisabled();
    await page.getByLabel("App name").fill("smoke-app");
    // Name alone isn't enough; script editor still empty → still disabled.
    await expect(review).toBeDisabled();
  });
});
