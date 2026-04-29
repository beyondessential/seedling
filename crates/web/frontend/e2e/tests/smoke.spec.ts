import { expect, test } from "../test-fixtures";

test("home page loads and shows hostname in title", async ({ page, stack }) => {
  await page.goto(stack.baseUrl);
  await expect(page).toHaveTitle(/Seedling/);
});

test("healthz responds", async ({ request, stack }) => {
  const res = await request.get(`${stack.baseUrl}/healthz`);
  expect(res.ok()).toBeTruthy();
});
