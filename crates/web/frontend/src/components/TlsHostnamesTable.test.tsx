import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { TlsHostnameView } from "../lib/types";
import { TlsHostnamesTable } from "./TlsHostnamesTable";

const now = Math.floor(Date.now() / 1000);

const acmeRow: TlsHostnameView = {
  hostname: "shop.example.com",
  apps: ["shop"],
  policy: {
    strategy: "acme_dns",
    dns_provider: "r53-main",
    pattern: "*.example.com",
    is_wildcard_match: true,
  },
  status: "active",
  // Bound to a policy naming one strategy, so on no chain.
  chain: null,
  active_cert: {
    id: 3,
    origin: "acme_dns",
    issuer: "CN=Fake LE",
    not_before: now - 86400,
    not_after: now + 30 * 86400,
    self_signed: false,
    ari_window_start: null,
    ari_window_end: null,
  },
  last_issuance: { kind: "acme_dns", at: now - 86400, cert_id: 3, provider: "r53-main" },
  last_error: null,
  retry_block: null,
  force_retry_at: null,
  next_issuance_at: now + 20 * 86400,
  next_issuance_source: "ari",
};

const defaultRow: TlsHostnameView = {
  hostname: "blog.example.com",
  apps: ["blog"],
  policy: { strategy: "default" },
  status: "default",
  chain: { responsible: "acme_http01", fall_through_at: null, next_cycle_at: now + 3 * 86400 },
  active_cert: {
    id: null,
    origin: "caddy",
    caddy_issuer: "local",
    issuer: null,
    not_before: now - 3600,
    not_after: now + 7 * 86400,
    self_signed: true,
    ari_window_start: null,
    ari_window_end: null,
  },
  last_issuance: { kind: "caddy", at: now - 3600, cert_id: null, provider: "local" },
  last_error: null,
  retry_block: null,
  force_retry_at: null,
  next_issuance_at: null,
  next_issuance_source: null,
};

const queuedRow: TlsHostnameView = {
  hostname: "api.example.com",
  apps: ["api"],
  policy: {
    strategy: "acme_dns",
    dns_provider: "r53-main",
    pattern: "api.example.com",
    is_wildcard_match: false,
  },
  status: "pending",
  chain: null,
  active_cert: null,
  last_issuance: null,
  last_error: null,
  retry_block: null,
  force_retry_at: now,
  next_issuance_at: now,
  next_issuance_source: "immediate",
};

const canopyRow: TlsHostnameView = {
  hostname: "portal.example.com",
  apps: ["portal"],
  policy: { strategy: "default" },
  status: "active",
  chain: {
    responsible: "canopy",
    fall_through_at: now + 5 * 86400,
    next_cycle_at: null,
  },
  active_cert: {
    id: 11,
    origin: "canopy",
    issuer: "CN=Fake LE",
    not_before: now - 70 * 86400,
    not_after: now + 20 * 86400,
    self_signed: false,
    ari_window_start: null,
    ari_window_end: null,
  },
  last_issuance: { kind: "canopy", at: now - 70 * 86400, cert_id: 11 },
  last_error: null,
  retry_block: null,
  force_retry_at: null,
  next_issuance_at: null,
  next_issuance_source: null,
};

const canopyOnlyRow: TlsHostnameView = {
  hostname: "later.example.com",
  apps: ["later"],
  policy: { strategy: "canopy", pattern: "*.example.com", is_wildcard_match: true },
  status: "pending",
  chain: { responsible: "canopy", fall_through_at: null, next_cycle_at: null },
  active_cert: null,
  last_issuance: null,
  last_error: null,
  retry_block: null,
  force_retry_at: null,
  next_issuance_at: null,
  next_issuance_source: "immediate",
};

const allRows = { hostnames: [acmeRow, defaultRow, queuedRow] };

/** The only role=button element in a hostname row is the renew icon button
 *  (app chips render as links). */
function renewButton(hostname: string): HTMLButtonElement {
  const row = screen.getByText(hostname).closest("tr");
  expect(row).not.toBeNull();
  return within(row as HTMLElement).getByRole("button") as HTMLButtonElement;
}

describe("TlsHostnamesTable", () => {
  it("renders the empty state", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": { hostnames: [] } },
    });
    expect(
      await screen.findByText("No TLS-terminating ingress domains declared."),
    ).toBeTruthy();
  });

  // spec: UI#certificates
  it("renders hostname rows with status, policy, and issuance details", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": allRows },
    });

    expect(await screen.findByText("shop.example.com")).toBeTruthy();
    const appChip = screen.getByRole("link", { name: "shop" });
    expect(appChip.getAttribute("href")).toBe("/apps/shop");

    // ACME-DNS row: wildcard-policy summary, last issuance, ARI schedule.
    const shopRow = screen.getByText("shop.example.com").closest("tr") as HTMLElement;
    expect(within(shopRow).getByText("ACME-DNS via r53-main (*.example.com)")).toBeTruthy();
    expect(within(shopRow).getByText("ACME-DNS via r53-main")).toBeTruthy();
    expect(within(shopRow).getByText("ARI")).toBeTruthy();

    // Default (Caddy-managed) row: proxy status, Caddy issuer, no schedule.
    expect(screen.getByText("default (proxy)")).toBeTruthy();
    expect(screen.getByText("default — Caddy internal CA")).toBeTruthy();
    expect(screen.getByText("controlled by Caddy")).toBeTruthy();
    expect(screen.getByText("self-signed")).toBeTruthy();

    // Immediate-issuance row.
    expect(screen.getByText("queued")).toBeTruthy();
  });

  // spec: UI#certificates
  it("shows the strategy responsible and when it falls through", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": { hostnames: [canopyRow, canopyOnlyRow, defaultRow] } },
    });
    await screen.findByText("portal.example.com");

    const portal = screen.getByText("portal.example.com").closest("tr") as HTMLElement;
    expect(within(portal).getByText("Canopy, falls through in 5d")).toBeTruthy();
    expect(within(portal).getByText("Canopy")).toBeTruthy();

    const later = screen.getByText("later.example.com").closest("tr") as HTMLElement;
    expect(within(later).getByText("Canopy only (*.example.com)")).toBeTruthy();
    expect(within(later).getByText("Canopy")).toBeTruthy();

    const blog = screen.getByText("blog.example.com").closest("tr") as HTMLElement;
    expect(within(blog).getByText("HTTP-01")).toBeTruthy();
  });

  it("names a Tailscale issuance as Tailscale", async () => {
    const tailscaleRow: TlsHostnameView = {
      ...defaultRow,
      hostname: "node.tail1234.ts.net",
      apps: ["node"],
      chain: null,
      active_cert: null,
      last_issuance: { kind: "tailscale", at: now - 3600, cert_id: 21 },
    };
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": { hostnames: [tailscaleRow] } },
    });
    const row = (await screen.findByText("node.tail1234.ts.net")).closest("tr") as HTMLElement;
    expect(within(row).getByText("Tailscale")).toBeTruthy();
    expect(within(row).queryByText(/ACME-DNS/)).toBeNull();
  });

  // spec: UI#certificates
  it("offers a retry wherever Canopy is the one being asked", async () => {
    const { request } = renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": { hostnames: [canopyRow, canopyOnlyRow, defaultRow] } },
      safetyMode: "write",
    });
    await screen.findByText("portal.example.com");
    expect(within(screen.getByText("blog.example.com").closest("tr") as HTMLElement).queryByRole("button")).toBeNull();
    fireEvent.click(renewButton("later.example.com"));
    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/retry", {
        hostname: "later.example.com",
      });
    });
    expect(renewButton("portal.example.com")).toBeTruthy();
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: {
        "/tls/hostnames/list": {
          ok: false,
          error: { code: "internal", message: "hostname rollup failed" },
        },
      },
    });
    expect(await screen.findByText(/hostname rollup failed/)).toBeTruthy();
  });

  it("passes the app filter to the query and hides the Apps column", async () => {
    const { request } = renderWithSession(
      <TlsHostnamesTable app="shop" hideAppsColumn />,
      { fixtures: { "/tls/hostnames/list": { hostnames: [acmeRow] } } },
    );
    expect(await screen.findByText("shop.example.com")).toBeTruthy();
    expect(request).toHaveBeenCalledWith("/tls/hostnames/list", { app: "shop" });
    expect(screen.queryByText("Apps")).toBeNull();
  });

  it("disables the renew button in read mode", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": allRows },
    });
    await screen.findByText("shop.example.com");
    expect(renewButton("shop.example.com").disabled).toBe(true);
  });

  it("issues a retry for an ACME-DNS hostname in write mode", async () => {
    const { request } = renderWithSession(<TlsHostnamesTable />, {
      fixtures: { "/tls/hostnames/list": allRows },
      safetyMode: "write",
    });
    await screen.findByText("shop.example.com");

    // The default-strategy row exposes no renew action at all.
    const defaultRowEl = screen.getByText("blog.example.com").closest("tr");
    expect(within(defaultRowEl as HTMLElement).queryByRole("button")).toBeNull();

    fireEvent.click(renewButton("shop.example.com"));
    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/retry", {
        hostname: "shop.example.com",
      });
    });
  });

  it("surfaces a retry failure inline", async () => {
    renderWithSession(<TlsHostnamesTable />, {
      fixtures: {
        "/tls/hostnames/list": allRows,
        "/tls/certificates/retry": {
          ok: false,
          error: { code: "conflict", message: "issuance already queued" },
        },
      },
      safetyMode: "write",
    });
    await screen.findByText("shop.example.com");
    fireEvent.click(renewButton("shop.example.com"));
    expect(await screen.findByText(/issuance already queued/)).toBeTruthy();
  });
});
