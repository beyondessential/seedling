// Visit each top-level route and verify the SPA renders the expected
// landmark heading without surfacing a render error. We click through the
// nav-bar icon links rather than driving the URL directly: that way we
// also catch broken navbar wiring (wrong link target, missing tooltip) at
// the same time.
//
// Each test verifies that the web interface provides the named route at
// the expected path; the spec items below cover broader management
// surfaces too, of which routing is one slice.
//
// w[verify routes.keys]
// w[verify routes.registries]
// w[verify routes.images]
// w[verify routes.certificates]
// w[verify routes.volumes]
// w[verify routes.backups]

import { expect, test } from "../test-fixtures";

const PAGES: Array<{ tooltip: RegExp; url: string; heading: RegExp }> = [
  { tooltip: /Authorised OI keys/i, url: "/keys", heading: /OI keys/i },
  { tooltip: /Container registry allowlist/i, url: "/registries", heading: /Registry/i },
  { tooltip: /Container images/i, url: "/images", heading: /Images/i },
  { tooltip: /^Services$/, url: "/services", heading: /Services/i },
  { tooltip: /Site ingresses/i, url: "/ingresses", heading: /Ingresses/i },
  { tooltip: /TLS certificates/i, url: "/certificates", heading: /Certificates/i },
  { tooltip: /^Volumes(?!:)/, url: "/volumes", heading: /Volumes/i },
  { tooltip: /^Backups$/, url: "/backups", heading: /Backups/i },
  { tooltip: /^Templates$/, url: "/templates", heading: /Templates/i },
];

test.describe("navigation", () => {
  test("home shows the Apps landing", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    await expect(page.getByRole("heading", { name: /^Apps$/i })).toBeVisible();
  });

  for (const { tooltip, url, heading } of PAGES) {
    test(`${url} loads via navbar`, async ({ page, stack }) => {
      await page.goto(stack.baseUrl);
      // Wait for the navbar to be hydrated before clicking — the tooltip
      // host element is the link, but it only mounts once the SPA boots.
      const link = page.getByRole("link", { name: tooltip });
      await link.waitFor();
      await link.click();
      await expect(page).toHaveURL(new RegExp(`${url}$`));
      await expect(page.getByRole("heading", { name: heading }).first()).toBeVisible();
    });
  }

  test("server hostname appears in the page title", async ({ page, stack }) => {
    await page.goto(stack.baseUrl);
    // Title shape: "<hostname> · Seedling". Wait until the hostname has
    // arrived (replacing the bare "Seedling" placeholder).
    await expect.poll(async () => await page.title()).toMatch(/.+ · Seedling$/);
  });
});
