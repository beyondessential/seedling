import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  TlsCertificate,
  TlsDnsProvider,
  TlsPolicy,
  TlsSettings,
} from "../lib/types";
import Certificates from "./Certificates";

const now = Math.floor(Date.now() / 1000);

const provider: TlsDnsProvider = {
  name: "r53-main",
  kind: "route53",
  created_at: now - 86400,
  updated_at: now - 3600,
};

const policy: TlsPolicy = {
  hostname: "*.example.com",
  strategy: "acme_dns",
  dns_provider: "r53-main",
  updated_at: now - 3600,
};

const settings: TlsSettings = {
  contact_email: "ops@example.com",
  cert_profile: null,
  updated_at: now - 3600,
};

const manualCert: TlsCertificate = {
  id: 3,
  hostname: "shop.example.com",
  state: "active",
  origin: "manual",
  key_type: "ecdsa_p256",
  issuer: "CN=Example CA",
  not_before: now - 86400,
  not_after: now + 30 * 86400,
  serial: "ab12cd34",
  self_signed: true,
  note: null,
  acme_account_id: null,
  created_at: now - 86400,
  updated_at: now - 86400,
};

const pendingCsr: TlsCertificate = {
  id: 5,
  hostname: "csr.example.com",
  state: "csr_pending",
  origin: "csr",
  key_type: "ecdsa_p256",
  issuer: null,
  not_before: null,
  not_after: null,
  serial: null,
  self_signed: false,
  note: null,
  acme_account_id: null,
  created_at: now - 600,
  updated_at: now - 600,
};

// ACME-DNS-issued certs must not appear in the manual/CSR section.
const acmeCert: TlsCertificate = {
  id: 8,
  hostname: "auto.example.com",
  state: "active",
  origin: "acme_dns",
  key_type: "ecdsa_p256",
  issuer: "CN=Fake LE",
  not_before: now - 86400,
  not_after: now + 60 * 86400,
  serial: "ef56",
  self_signed: false,
  note: null,
  acme_account_id: 1,
  created_at: now - 86400,
  updated_at: now - 86400,
};

const CERT_PEM = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
const KEY_PEM = "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n";
const CSR_PEM =
  "-----BEGIN CERTIFICATE REQUEST-----\nMIIC\n-----END CERTIFICATE REQUEST-----\n";

function baseFixtures(overrides: Record<string, unknown> = {}) {
  return {
    "/tls/hostnames/list": { hostnames: [] },
    "/tls/settings/get": settings,
    "/tls/policies/list": { policies: [policy] },
    "/tls/dns-providers/list": { providers: [provider] },
    "/tls/certificates/list": { certificates: [manualCert, pendingCsr, acmeCert] },
    ...overrides,
  };
}

/** The only role=button element in a policy/provider/stored-cert row (other
 *  than any named text buttons) is its unlabelled row-action icon button. */
function rowFor(text: string): HTMLElement {
  const row = screen.getByText(text).closest("tr");
  expect(row).not.toBeNull();
  return row as HTMLElement;
}

/** getByDisplayValue normalises whitespace, which mangles multi-line PEM
 *  bodies — read the textarea values directly instead. */
function textareaValues(): string[] {
  return Array.from(document.querySelectorAll("textarea")).map((t) => t.value);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("Certificates", () => {
  // w[verify routes.certificates]
  it("renders settings, policies, stored certificates, and providers", async () => {
    renderWithSession(<Certificates />, { fixtures: baseFixtures() });

    // Settings.
    expect(await screen.findByText("ops@example.com")).toBeTruthy();
    expect(
      screen.getByText("default (CA picks the profile, ~90 days at Let's Encrypt)"),
    ).toBeTruthy();

    // Policies.
    expect(screen.getByText("*.example.com")).toBeTruthy();
    expect(screen.getByText("provider: r53-main")).toBeTruthy();
    expect(screen.getByText("acme-dns")).toBeTruthy();

    // Stored certificates: manual cert with state/origin/self-signed flags.
    const certRow = rowFor("shop.example.com");
    expect(within(certRow).getByText("manual")).toBeTruthy();
    expect(within(certRow).getByText("active")).toBeTruthy();
    expect(within(certRow).getByText("self-signed")).toBeTruthy();
    expect(within(certRow).getByText("CN=Example CA")).toBeTruthy();

    // Pending CSR row exposes the CSR actions instead of delete.
    const csrRow = rowFor("csr.example.com");
    expect(within(csrRow).getByRole("button", { name: "Show CSR" })).toBeTruthy();
    expect(within(csrRow).getByRole("button", { name: "Upload cert" })).toBeTruthy();

    // ACME-DNS-issued certs are excluded from the manual section.
    expect(screen.queryByText("auto.example.com")).toBeNull();

    // Providers list shows name and kind, never credentials.
    const providerRow = rowFor("r53-main");
    expect(within(providerRow).getByText("route53")).toBeTruthy();
  });

  it("renders the empty states", async () => {
    renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/settings/get": { ...settings, contact_email: "" },
        "/tls/policies/list": { policies: [] },
        "/tls/dns-providers/list": { providers: [] },
        "/tls/certificates/list": { certificates: [] },
      }),
    });

    expect(await screen.findByText("not set")).toBeTruthy();
    expect(
      screen.getByText(
        "No operator policies — every TLS-terminating domain uses the Caddy default.",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText("No manual or CSR-derived certificates stored."),
    ).toBeTruthy();
    expect(
      screen.getByText("No DNS providers configured. Add one to enable ACME-DNS-01."),
    ).toBeTruthy();
  });

  it("shows an error alert per failing section query", async () => {
    renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/settings/get": {
          ok: false,
          error: { code: "internal", message: "settings broke" },
        },
        "/tls/policies/list": {
          ok: false,
          error: { code: "internal", message: "policies broke" },
        },
        "/tls/dns-providers/list": {
          ok: false,
          error: { code: "internal", message: "providers broke" },
        },
        "/tls/certificates/list": {
          ok: false,
          error: { code: "internal", message: "certs broke" },
        },
      }),
    });

    expect(await screen.findByText(/settings broke/)).toBeTruthy();
    expect(await screen.findByText(/policies broke/)).toBeTruthy();
    expect(await screen.findByText(/providers broke/)).toBeTruthy();
    expect(await screen.findByText(/certs broke/)).toBeTruthy();
  });

  it("disables mutating buttons in read mode", async () => {
    renderWithSession(<Certificates />, { fixtures: baseFixtures() });

    const edit = (await screen.findByRole("button", {
      name: "Edit",
    })) as HTMLButtonElement;
    expect(edit.disabled).toBe(true);
    for (const name of ["Bind domain", "Generate CSR", "Upload manual cert", "Add"]) {
      const btn = screen.getByRole("button", { name }) as HTMLButtonElement;
      expect(btn.disabled).toBe(true);
    }
  });

  it("saves TLS settings with the short-lived profile", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const email = await screen.findByLabelText("Contact email");
    fireEvent.change(email, { target: { value: " new-ops@example.com " } });
    fireEvent.click(screen.getByLabelText(/short-lived \(~6-day\) certificates/));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/settings/set", {
        contact_email: "new-ops@example.com",
        cert_profile: "shortlived",
      });
    });
  });

  // w[verify routes.certificates]
  it("binds a domain to ACME-DNS via the policy dialog", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Bind domain" }));
    const host = await screen.findByLabelText("Domain or wildcard");
    fireEvent.change(host, { target: { value: "api.example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/policies/set-acme-dns", {
        hostname: "api.example.com",
        dns_provider: "r53-main",
      });
    });
  });

  // w[verify routes.certificates]
  it("clears a policy after confirmation", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "write",
    });

    await screen.findByText("*.example.com");
    fireEvent.click(within(rowFor("*.example.com")).getByRole("button"));

    expect(await screen.findByText("Clear policy")).toBeTruthy();
    expect(screen.getByText(/Clear the policy for "\*\.example\.com"/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/policies/clear", {
        hostname: "*.example.com",
      });
    });
  });

  // w[verify routes.certificates]
  it("adds a Route 53 DNS provider", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Add" }));
    fireEvent.change(await screen.findByLabelText("Name"), {
      target: { value: "ops-account" },
    });
    fireEvent.change(screen.getByLabelText("Access key ID"), {
      target: { value: "AKIAEXAMPLE" },
    });
    fireEvent.change(screen.getByLabelText("Secret access key"), {
      target: { value: "s3cr3t" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/dns-providers/upsert", {
        name: "ops-account",
        kind: "route53",
        config: {
          access_key_id: "AKIAEXAMPLE",
          secret_access_key: "s3cr3t",
          region: "us-east-1",
        },
      });
    });
  });

  // w[verify routes.certificates]
  it("deletes a DNS provider after confirmation in dangerous mode", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "dangerous",
    });

    await screen.findByText("r53-main");
    fireEvent.click(within(rowFor("r53-main")).getByRole("button"));

    expect(await screen.findByText("Delete DNS provider")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/dns-providers/delete", {
        name: "r53-main",
      });
    });
  });

  it("deletes a stored certificate after confirmation in dangerous mode", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "dangerous",
    });

    await screen.findByText("shop.example.com");
    fireEvent.click(within(rowFor("shop.example.com")).getByRole("button"));

    expect(await screen.findByText("Delete certificate")).toBeTruthy();
    expect(
      screen.getByText(/Delete cert #3 \(shop\.example\.com\)\?/),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/delete", { id: 3 });
    });
  });

  it("uploads a manual certificate and closes on a clean result", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/certificates/upload-manual": {
          primary_san: "shop.example.com",
          san_dns_names: ["shop.example.com"],
          warnings: [],
        },
      }),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Upload manual cert" }));
    fireEvent.change(
      await screen.findByPlaceholderText("-----BEGIN CERTIFICATE-----..."),
      { target: { value: CERT_PEM } },
    );
    fireEvent.change(screen.getByPlaceholderText("-----BEGIN PRIVATE KEY-----..."), {
      target: { value: KEY_PEM },
    });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/upload-manual", {
        cert_pem: CERT_PEM,
        key_pem: KEY_PEM,
      });
    });
    await waitFor(() => {
      expect(screen.queryByText("Upload manual certificate")).toBeNull();
    });
  });

  it("keeps the manual upload dialog open to show warnings", async () => {
    renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/certificates/upload-manual": {
          primary_san: "shop.example.com",
          san_dns_names: ["shop.example.com", "www.example.com"],
          warnings: ["certificate expires in 3 days"],
        },
      }),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Upload manual cert" }));
    fireEvent.change(
      await screen.findByPlaceholderText("-----BEGIN CERTIFICATE-----..."),
      { target: { value: CERT_PEM } },
    );
    fireEvent.change(screen.getByPlaceholderText("-----BEGIN PRIVATE KEY-----..."), {
      target: { value: KEY_PEM },
    });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));

    expect(
      await screen.findByText(/Uploaded with warnings: certificate expires in 3 days/),
    ).toBeTruthy();
    expect(
      screen.getByText(/Will cover: shop\.example\.com, www\.example\.com/),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "OK" })).toBeTruthy();
  });

  it("generates a CSR, shows the PEM, and supports copy and download", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const createObjectURL = vi.fn(() => "blob:test");
    const revokeObjectURL = vi.fn();
    Object.assign(URL, { createObjectURL, revokeObjectURL });
    const anchorClick = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => undefined);

    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/certificates/csr/begin": { id: 9, csr_pem: CSR_PEM },
      }),
      safetyMode: "write",
    });

    fireEvent.click(await screen.findByRole("button", { name: "Generate CSR" }));
    fireEvent.change(await screen.findByLabelText("Hostname"), {
      target: { value: "csr2.example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Generate" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/csr/begin", {
        hostname: "csr2.example.com",
        key_type: "ecdsa_p256",
      });
    });
    expect(await screen.findByText("Pending CSR #9")).toBeTruthy();
    expect(textareaValues()).toContain(CSR_PEM);

    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(CSR_PEM));

    fireEvent.click(screen.getByRole("button", { name: "Download" }));
    expect(createObjectURL).toHaveBeenCalled();
    expect(anchorClick).toHaveBeenCalled();
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:test");
  });

  it("re-shows the CSR PEM for a pending certificate", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/certificates/csr/get": { id: 5, csr_pem: CSR_PEM },
      }),
    });

    await screen.findByText("csr.example.com");
    fireEvent.click(screen.getByRole("button", { name: "Show CSR" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/csr/get", { id: 5 });
    });
    expect(await screen.findByText("Pending CSR #5")).toBeTruthy();
    expect(textareaValues()).toContain(CSR_PEM);
  });

  it("uploads the signed certificate for a pending CSR", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures({
        "/tls/certificates/csr/upload-cert": { warnings: [] },
      }),
      safetyMode: "write",
    });

    await screen.findByText("csr.example.com");
    fireEvent.click(screen.getByRole("button", { name: "Upload cert" }));

    expect(
      await screen.findByText("Upload signed cert for csr.example.com (#5)"),
    ).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText("-----BEGIN CERTIFICATE-----..."), {
      target: { value: CERT_PEM },
    });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/csr/upload-cert", {
        id: 5,
        cert_pem: CERT_PEM,
      });
    });
  });

  it("cancels a pending CSR from the row action", async () => {
    const { request } = renderWithSession(<Certificates />, {
      fixtures: baseFixtures(),
      safetyMode: "write",
    });

    await screen.findByText("csr.example.com");
    // The pending row's buttons are Show CSR, Upload cert, then the
    // unlabelled cancel icon button.
    const buttons = within(rowFor("csr.example.com")).getAllByRole("button");
    expect(buttons).toHaveLength(3);
    fireEvent.click(buttons[2]);

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith("/tls/certificates/csr/cancel", { id: 5 });
    });
  });
});
