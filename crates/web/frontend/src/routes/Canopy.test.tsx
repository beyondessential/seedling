import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import Canopy from "./Canopy";

const offered = {
  enabled: true,
  offer: {
    offer_id: "018f0000-0000-7000-8000-000000000001",
    agent: "bestool 0.7.8",
    endpoint: "https://meta.example.invalid",
    via: "mtls",
    offered_at: "2026-07-30T08:00:00Z",
  },
  offers: 1,
  server_id: "018f0000-0000-7000-8000-0000000000aa",
  last_report: { at: "2026-07-30T08:01:00Z", ok: true },
};

const nothingOffering = {
  enabled: true,
  offer: null,
  offers: 0,
  server_id: null,
  last_report: null,
};

describe("Canopy", () => {
  // w[verify routes.canopy]
  it("shows the carrying client and the endpoint it reaches", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": offered },
    });
    expect(await screen.findByText("bestool 0.7.8")).toBeTruthy();
    expect(screen.getByText("https://meta.example.invalid")).toBeTruthy();
    expect(screen.getByText("mtls")).toBeTruthy();
    expect(screen.getByText("2026-07-30T08:00:00Z")).toBeTruthy();
  });

  // w[verify routes.canopy]
  it("says nothing is wrong when no client is offering", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": nothingOffering },
    });
    expect(
      await screen.findByText(/No client is currently offering/),
    ).toBeTruthy();
    // A host without bestool is a deployment choice, so the page must not
    // present the absence as a problem.
    expect(screen.getByText(/nothing is wrong/)).toBeTruthy();
  });

  // w[verify routes.canopy]
  it("shows whether access is enabled", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": offered },
    });
    expect(await screen.findByText("Enabled")).toBeTruthy();
  });

  // w[verify routes.canopy]
  it("turns access off through the settings method", async () => {
    const { request } = renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": offered },
      safetyMode: "dangerous",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Disable" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/canopy/settings/set",
        { enabled: false },
      ]),
    );
  });

  // w[verify routes.canopy]
  it("turns access back on through the same method", async () => {
    const { request } = renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": { ...nothingOffering, enabled: false } },
      safetyMode: "write",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Enable" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/canopy/settings/set",
        { enabled: true },
      ]),
    );
  });

  // w[verify routes.canopy]
  it("reports on demand", async () => {
    const { request } = renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": offered },
      safetyMode: "write",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Report now" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual(["/canopy/report", {}]),
    );
  });

  // w[verify routes.canopy]
  it("does not offer to report when there is nothing to report through", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": nothingOffering },
      safetyMode: "write",
    });
    const button = await screen.findByRole("button", { name: "Report now" });
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  // w[verify routes.canopy]
  it("shows the error from a failed report", async () => {
    renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": {
          ...offered,
          last_report: {
            at: "2026-07-30T08:01:00Z",
            ok: false,
            error: "canopy returned 401",
          },
        },
      },
    });
    expect(await screen.findByText("Failed")).toBeTruthy();
    expect(screen.getByText("canopy returned 401")).toBeTruthy();
  });

  // w[verify routes.canopy]
  it("says so when the server identity has not been resolved yet", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": nothingOffering },
    });
    expect(await screen.findByText("not yet resolved")).toBeTruthy();
  });

  it("mentions the older offers that would take over", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": { ...offered, offers: 3 } },
    });
    expect(await screen.findByText(/2 older offers/)).toBeTruthy();
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": {
          ok: false,
          error: { code: "internal", message: "db exploded" },
        },
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });
});
