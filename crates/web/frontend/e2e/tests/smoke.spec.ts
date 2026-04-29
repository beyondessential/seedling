import { expect, test } from "@playwright/test";

import { startStack, type StackHandle } from "../fixture";

let stack: StackHandle;

test.beforeAll(async () => {
  stack = await startStack();
});

test.afterAll(async () => {
  await stack?.stop();
});

test("home page loads and shows hostname in title", async ({ page }) => {
  await page.goto(stack.baseUrl);
  await expect(page).toHaveTitle(/Seedling/);
});

test("healthz responds", async ({ request }) => {
  const res = await request.get(`${stack.baseUrl}/healthz`);
  expect(res.ok()).toBeTruthy();
});
