import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { ImagePin, ImageSummary } from "../lib/types";
import Images from "./Images";

const imgInUse: ImageSummary = {
  image_id: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  tags: ["docker.io/library/nginx:1.27"],
  digests: [],
  manifest_digest: null,
  size_bytes: 2 * 1024 * 1024 * 1024,
  created_at: "2026-07-01T00:00:00Z",
  last_used_at: "2026-07-09T00:00:00Z",
  in_use: true,
  pinned_by: [],
};

const imgUnused: ImageSummary = {
  image_id: "sha256:2222222222222222222222222222222222222222222222222222222222222222",
  tags: ["ghcr.io/acme/tool:2.0"],
  digests: [],
  manifest_digest: null,
  size_bytes: 512,
  created_at: "2026-06-01T00:00:00Z",
  last_used_at: "2026-06-15T00:00:00Z",
  in_use: false,
  pinned_by: [],
};

const imgPinned: ImageSummary = {
  image_id: "sha256:3333333333333333333333333333333333333333333333333333333333333333",
  tags: ["ghcr.io/acme/app:latest"],
  digests: [],
  manifest_digest: null,
  size_bytes: 5 * 1024 * 1024,
  created_at: "2026-06-20T00:00:00Z",
  last_used_at: "2026-07-08T00:00:00Z",
  in_use: false,
  pinned_by: ["shop"],
};

const pin: ImagePin = {
  app: "shop",
  reference: "ghcr.io/acme/app:latest",
  pinned_at: "2026-07-08T00:00:00Z",
  expires_at: null,
};

const fixtures = {
  "/images/list": { images: [imgInUse, imgUnused, imgPinned] },
  "/images/pins/list": { pins: [pin] },
};

describe("Images", () => {
  // w[verify routes.images]
  it("renders the empty states for images and pins", async () => {
    renderWithSession(<Images />, {
      fixtures: { "/images/list": { images: [] }, "/images/pins/list": { pins: [] } },
    });
    expect(await screen.findByText("No container images in local storage.")).toBeTruthy();
    expect(await screen.findByText("No image pins.")).toBeTruthy();
  });

  // w[verify routes.images]
  it("renders image rows with sizes, state chips, and pin rows", async () => {
    renderWithSession(<Images />, { fixtures });
    expect(await screen.findByText("docker.io/library/nginx:1.27")).toBeTruthy();
    expect(screen.getByText("2.00 GiB")).toBeTruthy();
    expect(screen.getByText("512 B")).toBeTruthy();
    expect(screen.getByText("in use")).toBeTruthy();
    expect(screen.getByText("unused")).toBeTruthy();
    expect(screen.getByText("pinned (1)")).toBeTruthy();
    const appLink = screen.getByRole("link", { name: "shop" });
    expect(appLink.getAttribute("href")).toBe("/apps/shop");
  });

  it("shows an error alert when the image list query fails", async () => {
    renderWithSession(<Images />, {
      fixtures: {
        "/images/list": { ok: false, error: { code: "internal", message: "podman died" } },
        "/images/pins/list": { pins: [] },
      },
    });
    expect(await screen.findByText(/podman died/)).toBeTruthy();
  });

  // w[verify routes.images]
  // w[verify routes.images.confirm]
  it("removes an unused image via /images/remove after confirmation", async () => {
    const { request } = renderWithSession(<Images />, {
      fixtures,
      safetyMode: "dangerous",
    });
    const row = (await screen.findByText("ghcr.io/acme/tool:2.0")).closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/images/remove",
        { reference: "ghcr.io/acme/tool:2.0" },
      ]),
    );
  });

  it("disables removal of an in-use image", async () => {
    renderWithSession(<Images />, { fixtures, safetyMode: "dangerous" });
    const row = (await screen.findByText("docker.io/library/nginx:1.27")).closest("tr")!;
    expect(within(row).getByRole("button")).toHaveProperty("disabled", true);
  });

  // w[verify routes.images]
  it("clears a pin via /images/pins/clear after confirmation", async () => {
    const { request } = renderWithSession(<Images />, {
      fixtures,
      safetyMode: "write",
    });
    const pinCell = await screen.findByText(pin.reference, { selector: "td" });
    const row = pinCell.closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    fireEvent.click(await screen.findByRole("button", { name: "Clear pin" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/images/pins/clear",
        { app: pin.app, reference: pin.reference },
      ]),
    );
  });

  // w[verify routes.images]
  it("bulk-clears only unused unpinned images", async () => {
    const { request } = renderWithSession(<Images />, {
      fixtures,
      safetyMode: "dangerous",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Clear unused" }));
    fireEvent.click(await screen.findByRole("button", { name: "Remove 1" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/images/remove",
        { reference: "ghcr.io/acme/tool:2.0" },
      ]),
    );
    // The in-use and pinned images must not be swept.
    const removals = request.mock.calls.filter(([m]) => m === "/images/remove");
    expect(removals).toHaveLength(1);
  });
});
