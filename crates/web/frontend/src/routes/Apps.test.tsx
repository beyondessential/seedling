import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { AppSummary, ConnectedClients } from "../lib/types";
import Apps from "./Apps";

const apps: AppSummary[] = [
  {
    name: "shop",
    status: "running",
    fault_count: 2,
    description: "An online shop",
  },
  { name: "blog", status: "degraded" },
];

const noClients: ConnectedClients = { web: [], shells: [], forwards: [], actors: [] };

const clients: ConnectedClients = {
  web: [
    {
      id: "ws-1",
      connected_at: "2026-07-09T08:00:00Z",
      last_seen: "2026-07-09T09:00:00Z",
      actor_kind: "user",
      actor_id: "felix",
      actor_display: "Felix",
      safety_mode: "write",
    },
  ],
  shells: [
    {
      session_id: "sh-1",
      app: "shop",
      name: "web",
      opened_at: "2026-07-09T08:30:00Z",
      actor: { kind: "user", id: "felix", display: "Felix" },
    },
  ],
  forwards: [
    {
      forward_id: "fw-1",
      app: "blog",
      service: "db",
      port: 5432,
      proto: "tcp",
      opened_at: "2026-07-09T08:45:00Z",
    },
  ],
  actors: [],
};

describe("Apps", () => {
  // w[verify routes.apps]
  it("renders the empty state", async () => {
    renderWithSession(<Apps />, {
      fixtures: { "/apps/list": [], "/connected-clients/list": noClients },
    });
    expect(await screen.findByText("No apps registered.")).toBeTruthy();
  });

  // w[verify routes.apps]
  // w[verify routes.apps.fault-count]
  it("renders app rows with status chips and fault counts", async () => {
    renderWithSession(<Apps />, {
      fixtures: { "/apps/list": apps, "/connected-clients/list": noClients },
    });
    expect(await screen.findByText("shop")).toBeTruthy();
    expect(screen.getByText("blog")).toBeTruthy();
    expect(screen.getByText("An online shop")).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.getByText("degraded")).toBeTruthy();
    expect(screen.getByText("2 faults")).toBeTruthy();
  });

  it("shows an error alert when the app list query fails", async () => {
    renderWithSession(<Apps />, {
      fixtures: {
        "/apps/list": { ok: false, error: { code: "internal", message: "db exploded" } },
        "/connected-clients/list": noClients,
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });

  it("renders active sessions with links back to the app", async () => {
    renderWithSession(<Apps />, {
      fixtures: { "/apps/list": apps, "/connected-clients/list": clients },
    });
    expect(await screen.findByText("Active Sessions")).toBeTruthy();
    const shellLink = screen.getByRole("link", { name: "shop" });
    expect(shellLink.getAttribute("href")).toBe("/apps/shop");
    const forwardLink = screen.getByRole("link", { name: "blog" });
    expect(forwardLink.getAttribute("href")).toBe("/apps/blog");
    expect(screen.getAllByText("Felix").length).toBeGreaterThan(0);
    expect(screen.getByText("5432")).toBeTruthy();
  });

  it("stops a shell via /shells/stop in dangerous mode", async () => {
    const { request } = renderWithSession(<Apps />, {
      fixtures: { "/apps/list": apps, "/connected-clients/list": clients },
      safetyMode: "dangerous",
    });
    const shellCell = await screen.findByText("web", { selector: "td" });
    const row = shellCell.closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual(["/shells/stop", { session_id: "sh-1" }]),
    );
  });

  it("keeps the stop-shell button disabled in read mode", async () => {
    renderWithSession(<Apps />, {
      fixtures: { "/apps/list": apps, "/connected-clients/list": clients },
    });
    const shellCell = await screen.findByText("web", { selector: "td" });
    const row = shellCell.closest("tr")!;
    expect(within(row).getByRole("button")).toHaveProperty("disabled", true);
  });
});
