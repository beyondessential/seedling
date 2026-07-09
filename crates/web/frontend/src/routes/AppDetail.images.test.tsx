import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { DiscoverResponse, ImagePin, ImageSummary } from "../lib/types";
import { renderWithSession } from "../test/harness";
import { baseFixtures, makeDetail } from "./AppDetail.fixtures";
import AppDetail from "./AppDetail";

const ROUTE = { route: "/apps/myapp", path: "/apps/:name" };

function mount(
  fixtures: Record<string, unknown>,
  safetyMode?: "read" | "write" | "dangerous",
) {
  return renderWithSession(<AppDetail />, { ...ROUTE, fixtures, safetyMode });
}

function callsTo(request: ReturnType<typeof mount>["request"], method: string) {
  return request.mock.calls.filter((c) => c[0] === method);
}

function makeImage(overrides: Partial<ImageSummary> = {}): ImageSummary {
  return {
    image_id: "sha256:0011223344556677",
    tags: ["docker.io/library/nginx:1.27"],
    digests: [],
    manifest_digest: null,
    size_bytes: 157286400,
    created_at: "2026-07-01T00:00:00Z",
    last_used_at: "2026-07-08T00:00:00Z",
    in_use: true,
    pinned_by: ["myapp"],
    ...overrides,
  };
}

const pin: ImagePin = {
  app: "myapp",
  reference: "docker.io/library/nginx:1.27",
  pinned_at: "2026-07-01T00:00:00Z",
  expires_at: null,
};

describe("AppDetail images", () => {
  // w[verify routes.images.app-detail]
  it("lists in-use and pinned images and clears all pins", async () => {
    const fixtures = {
      ...baseFixtures(makeDetail()),
      "/images/list": { images: [makeImage()] },
      "/images/pins/list": { pins: [pin] },
    };
    const { request } = mount(fixtures, "write");
    expect(
      await screen.findByText("docker.io/library/nginx:1.27"),
    ).toBeTruthy();
    expect(screen.getByText("in use")).toBeTruthy();
    expect(screen.getByText("pinned")).toBeTruthy();
    expect(screen.getByText("150.0 MiB")).toBeTruthy();
    // In-use images cannot be removed.
    const remove = within(
      screen.getByLabelText("Cannot remove: image is in use"),
    ).getByRole("button") as HTMLButtonElement;
    expect(remove.disabled).toBe(true);
    // Clear all pins goes through a confirm dialog.
    fireEvent.click(screen.getByRole("button", { name: "Clear all pins" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/no longer protected/)).toBeTruthy();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Clear all pins" }),
    );
    await waitFor(() =>
      expect(callsTo(request, "/images/pins/clear")).toEqual([
        ["/images/pins/clear", { app: "myapp" }],
      ]),
    );
  });

  // w[verify routes.images.confirm]
  it("confirms before removing an unused image", async () => {
    const unused = makeImage({
      image_id: "sha256:8899aabbccddeeff",
      tags: ["ghcr.io/acme/tool:2"],
      in_use: false,
    });
    const fixtures = {
      ...baseFixtures(makeDetail()),
      "/images/list": { images: [unused] },
      "/images/pins/list": {
        pins: [{ ...pin, reference: "ghcr.io/acme/tool:2" }],
      },
    };
    const { request } = mount(fixtures, "dangerous");
    expect(await screen.findByText("ghcr.io/acme/tool:2")).toBeTruthy();
    fireEvent.click(
      within(screen.getByLabelText("Remove")).getByRole("button"),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText(
        /will fail if a running container is using the image/,
      ),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(callsTo(request, "/images/remove")).toEqual([
        ["/images/remove", { reference: "ghcr.io/acme/tool:2" }],
      ]),
    );
  });

  // w[verify routes.images.discover]
  it("discovers handler images, surfaces probe problems, and warms a reference", async () => {
    const discover: DiscoverResponse = {
      per_handler: [
        {
          name: "seed",
          kind: "action",
          images: [],
          error: "probe failed: params required",
          skipped_reason: null,
        },
      ],
      all_images: ["ghcr.io/acme/tool:2"],
    };
    const fixtures = {
      ...baseFixtures(makeDetail()),
      "/apps/images/discover": discover,
    };
    const { request } = mount(fixtures, "write");
    // With no in-use or pinned images the section starts as just the
    // discover affordance, without a table.
    const discoverBtn = await screen.findByRole("button", {
      name: "Discover from handlers",
    });
    expect(screen.queryByText("Reference")).toBeNull();
    fireEvent.click(discoverBtn);
    await waitFor(() =>
      expect(callsTo(request, "/apps/images/discover")).toEqual([
        ["/apps/images/discover", { app: "myapp", lenient: true }],
      ]),
    );
    // Discovered reference shows as a third, "potentially used" state.
    expect(await screen.findByText("ghcr.io/acme/tool:2")).toBeTruthy();
    expect(screen.getByText("potentially used")).toBeTruthy();
    expect(screen.getByText("not present")).toBeTruthy();
    // The failed probe is surfaced inline.
    expect(
      screen.getByText(/action\/seed: probe failed: params required/),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Warm all discovered" }),
    ).toBeTruthy();
    // Warming a single reference pulls and pins it to this app.
    fireEvent.click(screen.getByRole("button", { name: "Warm" }));
    await waitFor(() =>
      expect(callsTo(request, "/images/pull")).toEqual([
        ["/images/pull", { reference: "ghcr.io/acme/tool:2", app: "myapp" }],
      ]),
    );
  });
});
