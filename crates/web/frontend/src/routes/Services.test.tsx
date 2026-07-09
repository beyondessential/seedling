import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  AppService,
  DeclaredExternalService,
  ExportedService,
  ExternalServiceMapping,
  SiteService,
  SiteServiceResolverStatus,
} from "../lib/types";
import Services from "./Services";

const siteSvc: SiteService = {
  name: "db",
  description: "shared postgres",
  created_at: "2026-07-01T00:00:00Z",
  endpoints: [
    {
      service_port: 5432,
      protocol: "tcp",
      remote_host: "db.example.com",
      remote_port: 5433,
    },
    {
      service_port: 6379,
      protocol: "tcp",
      remote_host: "10.0.0.9",
      remote_port: 6379,
    },
  ],
};

const resolverStatus: SiteServiceResolverStatus = {
  entries: [
    {
      host: "db.example.com",
      aaaa: ["2001:db8::1"],
      a: [],
      last_attempt_failed: false,
      age_seconds: 5,
      ttl_remaining_seconds: 55,
    },
  ],
};

const exportedSvc: ExportedService = {
  app: "shop",
  service_name: "web",
  http: true,
  description: "storefront",
};

const appSvc: AppService = {
  app: "shop",
  service_name: "web",
  http: true,
  exported: true,
};

const declaredMapped: DeclaredExternalService = { app: "shop", name: "database" };
const declaredUnmapped: DeclaredExternalService = { app: "blog", name: "cache" };

const mapping: ExternalServiceMapping = {
  app: "shop",
  external_name: "database",
  target: { kind: "site", name: "db" },
};

const populated = {
  "/services/site/list": [siteSvc],
  "/services/exported/list": [exportedSvc],
  "/services/external/list": [mapping],
  "/services/external/declared": [declaredMapped, declaredUnmapped],
  "/services/site/resolver-status": resolverStatus,
  "/services/app/list": [appSvc],
};

const empty = {
  "/services/site/list": [],
  "/services/exported/list": [],
  "/services/external/list": [],
  "/services/external/declared": [],
  "/services/site/resolver-status": { entries: [] },
  "/services/app/list": [],
};

/** Find the enabled button wrapped by an ActionButton tooltip label. */
function buttonByTooltip(scope: HTMLElement, label: string): HTMLButtonElement {
  const holder = within(scope).getByLabelText(label);
  const btn =
    holder.tagName === "BUTTON"
      ? (holder as HTMLButtonElement)
      : holder.querySelector("button");
  if (!btn) throw new Error(`no button under tooltip ${label}`);
  return btn;
}

describe("Services", () => {
  it("shows spinners while loading, then the empty states", async () => {
    const { container } = renderWithSession(<Services />, { fixtures: empty });
    expect(container.querySelector(".MuiCircularProgress-root")).not.toBeNull();
    expect(await screen.findByText("No site services.")).toBeTruthy();
    expect(screen.getByText("No exported services.")).toBeTruthy();
    expect(
      screen.getByText("No external service slots declared across registered apps."),
    ).toBeTruthy();
  });

  it("renders site services, exports, and external slots when populated", async () => {
    renderWithSession(<Services />, { fixtures: populated });

    // Site service card with endpoints and resolver badge for the DNS host.
    expect(await screen.findByText("db")).toBeTruthy();
    expect(screen.getByText(/shared postgres/)).toBeTruthy();
    expect(screen.getByText("db.example.com:5433")).toBeTruthy();
    expect(screen.getByText("10.0.0.9:6379")).toBeTruthy();
    expect(screen.getAllByText("resolved")).toHaveLength(1);

    // App exports table with app link.
    const links = screen.getAllByRole("link", { name: "shop" });
    expect(links.length).toBeGreaterThanOrEqual(1);
    expect(links[0].getAttribute("href")).toBe("/apps/shop");
    expect(screen.getByText("storefront")).toBeTruthy();

    // External slots: mapped shows the target, unmapped shows the warning.
    expect(screen.getByText("_site/db")).toBeTruthy();
    expect(screen.getByText("unmapped")).toBeTruthy();
  });

  it("shows error alerts when the queries fail", async () => {
    renderWithSession(<Services />, {
      fixtures: {
        ...empty,
        "/services/site/list": {
          ok: false,
          error: { code: "internal", message: "site list broke" },
        },
        "/services/exported/list": {
          ok: false,
          error: { code: "internal", message: "exports broke" },
        },
      },
    });
    expect(await screen.findByText(/site list broke/)).toBeTruthy();
    expect(screen.getByText(/exports broke/)).toBeTruthy();
  });

  it("keeps guarded buttons disabled in read mode", async () => {
    renderWithSession(<Services />, { fixtures: populated });
    await screen.findByText("db");
    const newBtn = screen.getByRole("button", { name: "New" });
    expect((newBtn as HTMLButtonElement).disabled).toBe(true);
  });

  it("creates a site service through the dialog", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: empty,
      safetyMode: "write",
    });
    await screen.findByText("No site services.");
    fireEvent.click(screen.getByRole("button", { name: "New" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "cache" },
    });
    fireEvent.change(within(dialog).getByLabelText("Description (optional)"), {
      target: { value: "redis" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/services/site/create");
    });
    const call = request.mock.calls.find(([m]) => m === "/services/site/create");
    expect(call?.[1]).toEqual({ name: "cache", description: "redis" });
  });

  it("adds an endpoint with the values entered in the dialog", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("db");
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Service port"), {
      target: { value: "8080" },
    });
    fireEvent.change(within(dialog).getByLabelText("Remote host"), {
      target: { value: "backend.example.com" },
    });
    fireEvent.change(within(dialog).getByLabelText("Remote port"), {
      target: { value: "9090" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add endpoint" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain(
        "/services/site/endpoint/add",
      );
    });
    const call = request.mock.calls.find(
      ([m]) => m === "/services/site/endpoint/add",
    );
    expect(call?.[1]).toEqual({
      name: "db",
      service_port: 8080,
      protocol: "tcp",
      remote_host: "backend.example.com",
      remote_port: 9090,
    });
  });

  it("rejects an out-of-range service port client-side", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("db");
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Service port"), {
      target: { value: "70000" },
    });
    fireEvent.change(within(dialog).getByLabelText("Remote host"), {
      target: { value: "backend.example.com" },
    });
    fireEvent.change(within(dialog).getByLabelText("Remote port"), {
      target: { value: "9090" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add endpoint" }));

    expect(await within(dialog).findByText("service port must be 1–65535")).toBeTruthy();
    expect(
      request.mock.calls.find(([m]) => m === "/services/site/endpoint/add"),
    ).toBeUndefined();
  });

  it("deletes a site service after confirmation", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("db");
    fireEvent.click(buttonByTooltip(document.body, "Delete"));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete site service?")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/services/site/delete");
    });
    const call = request.mock.calls.find(([m]) => m === "/services/site/delete");
    expect(call?.[1]).toEqual({ name: "db" });
  });

  it("surfaces a failed site service deletion", async () => {
    renderWithSession(<Services />, {
      fixtures: {
        ...populated,
        "/services/site/delete": {
          ok: false,
          error: { code: "internal", message: "db exploded" },
        },
      },
      safetyMode: "dangerous",
    });
    await screen.findByText("db");
    fireEvent.click(buttonByTooltip(document.body, "Delete"));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });

  it("removes an endpoint from its row", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    const row = (await screen.findByText("db.example.com:5433")).closest("tr");
    expect(row).not.toBeNull();
    fireEvent.click(buttonByTooltip(row as HTMLElement, "Remove"));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain(
        "/services/site/endpoint/remove",
      );
    });
    const call = request.mock.calls.find(
      ([m]) => m === "/services/site/endpoint/remove",
    );
    expect(call?.[1]).toEqual({
      name: "db",
      service_port: 5432,
      protocol: "tcp",
      remote_host: "db.example.com",
      remote_port: 5433,
    });
  });

  it("unmaps an external service slot", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: populated,
      safetyMode: "write",
    });
    const row = (await screen.findByText("_site/db")).closest("tr");
    expect(row).not.toBeNull();
    fireEvent.click(buttonByTooltip(row as HTMLElement, "Unmap"));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain(
        "/services/external/unmap",
      );
    });
    const call = request.mock.calls.find(([m]) => m === "/services/external/unmap");
    expect(call?.[1]).toEqual({ app: "shop", external_name: "database" });
  });

  it("maps an unmapped slot to a site service via the prefilled dialog", async () => {
    const { request } = renderWithSession(<Services />, {
      fixtures: {
        ...populated,
        // No site services registered: the dialog falls back to a free-text
        // site-service name field, which is simpler to drive than the Select.
        "/services/site/list": [],
      },
      safetyMode: "write",
    });
    const row = (await screen.findByText("cache")).closest("tr");
    expect(row).not.toBeNull();
    fireEvent.click(within(row as HTMLElement).getByRole("button", { name: "Map" }));

    const dialog = await screen.findByRole("dialog");
    // Slot is prefilled and locked from the row that opened the dialog.
    expect(within(dialog).getByDisplayValue("blog / cache")).toBeTruthy();
    fireEvent.change(within(dialog).getByLabelText("Site service name"), {
      target: { value: "memcached" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Map" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/services/external/map");
    });
    const call = request.mock.calls.find(([m]) => m === "/services/external/map");
    expect(call?.[1]).toEqual({
      app: "blog",
      external_name: "cache",
      target: { kind: "site", name: "memcached" },
    });
  });

  it("surfaces an OI error inside the create dialog", async () => {
    renderWithSession(<Services />, {
      fixtures: {
        ...empty,
        "/services/site/create": {
          ok: false,
          error: { code: "conflict", message: "name already taken" },
        },
      },
      safetyMode: "write",
    });
    await screen.findByText("No site services.");
    fireEvent.click(screen.getByRole("button", { name: "New" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "cache" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    expect(await within(dialog).findByText(/name already taken/)).toBeTruthy();
  });
});
