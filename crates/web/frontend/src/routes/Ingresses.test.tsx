import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  AppService,
  SiteIngress,
  SiteIngressDiscoveryStatus,
} from "../lib/types";
import Ingresses from "./Ingresses";

const manualIngress: SiteIngress = {
  name: "old-site",
  hostname: "old.example.com",
  source: "manual",
  tls_provider: "acme",
  stale: false,
  created_at: "2026-07-01T00:00:00Z",
  attachments: [
    {
      port: 443,
      protocol: "http",
      target_kind: "forward",
      target_app: "shop",
      target_service: "web",
      created_at: "2026-07-01T00:00:00Z",
    },
    {
      port: 80,
      protocol: "http",
      target_kind: "redirect",
      redirect_url: "https://new.example.com",
      redirect_code: 308,
      redirect_preserve_path: true,
      created_at: "2026-07-01T00:00:00Z",
    },
  ],
};

const discoveredIngress: SiteIngress = {
  name: "ts-shop",
  hostname: "shop.tail.ts.net",
  source: "discovered",
  discovered_provider: "tailscale",
  discovered_key: "ts:shop",
  tls_provider: "tailscale",
  stale: true,
  created_at: "2026-07-01T00:00:00Z",
  attachments: [],
};

const staleDiscovery: SiteIngressDiscoveryStatus = {
  providers: [
    {
      name: "tailscale",
      ingresses: [
        {
          name: "ts-shop",
          provider: "tailscale",
          key: "ts:shop",
          hostname: "shop.tail.ts.net",
          stale: true,
        },
      ],
    },
  ],
};

const appSvc: AppService = {
  app: "shop",
  service_name: "web",
  http: true,
  exported: true,
};

const populated = {
  "/ingresses/site/list": [manualIngress, discoveredIngress],
  "/ingresses/site/discovery/status": staleDiscovery,
  "/services/app/list": [appSvc],
};

const empty = {
  "/ingresses/site/list": [],
  "/ingresses/site/discovery/status": { providers: [] },
  "/services/app/list": [],
};

/** Find the button wrapped by an ActionButton tooltip label. */
function buttonByTooltip(scope: HTMLElement, label: string): HTMLButtonElement {
  const holder = within(scope).getByLabelText(label);
  const btn =
    holder.tagName === "BUTTON"
      ? (holder as HTMLButtonElement)
      : holder.querySelector("button");
  if (!btn) throw new Error(`no button under tooltip ${label}`);
  return btn;
}

function rowOf(text: string): HTMLElement {
  const row = screen.getByText(text).closest("tr");
  if (!row) throw new Error(`no row containing ${text}`);
  return row;
}

describe("Ingresses", () => {
  it("shows a spinner while loading, then empty sections", async () => {
    const { container } = renderWithSession(<Ingresses />, { fixtures: empty });
    expect(container.querySelector(".MuiCircularProgress-root")).not.toBeNull();
    expect(await screen.findAllByText("(none)")).toHaveLength(2);
  });

  it("splits manual and discovered ingresses and renders attachments", async () => {
    renderWithSession(<Ingresses />, { fixtures: populated });

    expect(await screen.findByText("old-site")).toBeTruthy();
    expect(screen.getByText("old.example.com")).toBeTruthy();
    expect(within(rowOf("old-site")).getByText("Manual")).toBeTruthy();
    expect(screen.getByText("Discovered · tailscale")).toBeTruthy();
    expect(within(rowOf("ts-shop")).getByText("Stale")).toBeTruthy();

    // Attachment rendering: forward target and redirect description.
    expect(screen.getByText("shop/web")).toBeTruthy();
    expect(screen.getByText("↦ https://new.example.com (308)")).toBeTruthy();
    expect(screen.getByText("(no attachments)")).toBeTruthy();

    // Tailscale discovery being stale surfaces the warning banner.
    expect(screen.getByText(/Tailscale discovery is currently unhealthy/)).toBeTruthy();
  });

  it("disables delete for discovered ingresses even in dangerous mode", async () => {
    renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("ts-shop");
    const row = rowOf("ts-shop");
    const deleteBtn = row.querySelector(
      '[data-testid="DeleteOutlineOutlinedIcon"]',
    )?.closest("button");
    expect(deleteBtn).not.toBeNull();
    expect((deleteBtn as HTMLButtonElement).disabled).toBe(true);
  });

  it("keeps the create button disabled in read mode", async () => {
    renderWithSession(<Ingresses />, { fixtures: empty });
    await screen.findAllByText("(none)");
    const btn = screen.getByRole("button", { name: "New ingress" });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
  });

  it("creates a manual ingress through the dialog", async () => {
    const { request } = renderWithSession(<Ingresses />, {
      fixtures: empty,
      safetyMode: "write",
    });
    await screen.findAllByText("(none)");
    fireEvent.click(screen.getByRole("button", { name: "New ingress" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "legacy" },
    });
    fireEvent.change(within(dialog).getByLabelText("Hostname"), {
      target: { value: "legacy.example.com" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/ingresses/site/create");
    });
    const call = request.mock.calls.find(([m]) => m === "/ingresses/site/create");
    expect(call?.[1]).toEqual({
      name: "legacy",
      hostname: "legacy.example.com",
      tls_provider: "acme",
    });
  });

  it("attaches a forward to an app service", async () => {
    const { request } = renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("old-site");
    fireEvent.click(
      buttonByTooltip(rowOf("old-site"), "Attach forward or redirect"),
    );

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Attach to old-site")).toBeTruthy();

    // Pick the target from the app-service Select (port 443 and protocol
    // "http" are the defaults).
    fireEvent.mouseDown(
      within(dialog).getByRole("combobox", { name: /Target app \/ service/ }),
    );
    fireEvent.click(await screen.findByRole("option", { name: /shop/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Attach" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain(
        "/ingresses/site/attach/forward",
      );
    });
    const call = request.mock.calls.find(
      ([m]) => m === "/ingresses/site/attach/forward",
    );
    expect(call?.[1]).toEqual({
      name: "old-site",
      port: 443,
      protocol: "http",
      target_app: "shop",
      target_service: "web",
    });
  });

  it("attaches a redirect with URL, code, and preserve-path", async () => {
    const { request } = renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("old-site");
    fireEvent.click(
      buttonByTooltip(rowOf("old-site"), "Attach forward or redirect"),
    );

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByLabelText("Redirect to URL"));
    fireEvent.change(within(dialog).getByLabelText("Port"), {
      target: { value: "80" },
    });
    fireEvent.change(within(dialog).getByLabelText("Redirect URL"), {
      target: { value: "https://new.example.com/" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Attach" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain(
        "/ingresses/site/attach/redirect",
      );
    });
    const call = request.mock.calls.find(
      ([m]) => m === "/ingresses/site/attach/redirect",
    );
    expect(call?.[1]).toEqual({
      name: "old-site",
      port: 80,
      protocol: "http",
      redirect_url: "https://new.example.com/",
      redirect_code: 307,
      preserve_path: true,
    });
  });

  it("keeps Attach disabled for a redirect URL without an http scheme", async () => {
    renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("old-site");
    fireEvent.click(
      buttonByTooltip(rowOf("old-site"), "Attach forward or redirect"),
    );

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByLabelText("Redirect to URL"));
    fireEvent.change(within(dialog).getByLabelText("Redirect URL"), {
      target: { value: "ftp://new.example.com/" },
    });
    const attach = within(dialog).getByRole("button", { name: "Attach" });
    expect((attach as HTMLButtonElement).disabled).toBe(true);
  });

  it("detaches an attachment from its row", async () => {
    const { request } = renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("old-site");
    // The redirect attachment (port 80) sits next to its own detach button.
    const attachmentBox = screen
      .getByText("↦ https://new.example.com (308)")
      .closest("div");
    expect(attachmentBox).not.toBeNull();
    fireEvent.click(buttonByTooltip(attachmentBox as HTMLElement, "Detach"));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/ingresses/site/detach");
    });
    const call = request.mock.calls.find(([m]) => m === "/ingresses/site/detach");
    expect(call?.[1]).toEqual({ name: "old-site", port: 80, protocol: "http" });
  });

  it("deletes a manual ingress after confirmation", async () => {
    const { request } = renderWithSession(<Ingresses />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("old-site");
    fireEvent.click(
      buttonByTooltip(rowOf("old-site"), "Delete this manual site ingress"),
    );

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete site ingress?")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(request.mock.calls.map(([m]) => m)).toContain("/ingresses/site/delete");
    });
    const call = request.mock.calls.find(([m]) => m === "/ingresses/site/delete");
    expect(call?.[1]).toEqual({ name: "old-site" });
  });

  it("surfaces an OI error inside the create dialog", async () => {
    renderWithSession(<Ingresses />, {
      fixtures: {
        ...empty,
        "/ingresses/site/create": {
          ok: false,
          error: { code: "conflict", message: "hostname already in use" },
        },
      },
      safetyMode: "write",
    });
    await screen.findAllByText("(none)");
    fireEvent.click(screen.getByRole("button", { name: "New ingress" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "legacy" },
    });
    fireEvent.change(within(dialog).getByLabelText("Hostname"), {
      target: { value: "legacy.example.com" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    expect(await within(dialog).findByText(/hostname already in use/)).toBeTruthy();
  });
});
