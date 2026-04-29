import { expect, test } from "../test-fixtures";

// w[verify spa.delivery]
// w[verify auth.dev]
test("home page loads and shows hostname in title", async ({ page, stack }) => {
  await page.goto(stack.baseUrl);
  // The SPA is delivered from the plain-HTTP endpoint and rendered without
  // any login interstitial, since the fixture starts seedling-web with
  // --dev-no-auth on a loopback bind.
  await expect(page).toHaveTitle(/Seedling/);
  await expect(page).not.toHaveURL(/\/login$/);
});

// w[verify transport.http]
test("healthz responds", async ({ request, stack }) => {
  const res = await request.get(`${stack.baseUrl}/healthz`);
  expect(res.ok()).toBeTruthy();
});
