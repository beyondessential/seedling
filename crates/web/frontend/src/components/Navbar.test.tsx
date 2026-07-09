import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  ConnectedClients,
  FaultRecord,
  HeldVolume,
  WebSession,
} from "../lib/types";
import { Navbar } from "./Navbar";

function webSession(id: string, safetyMode: WebSession["safety_mode"]): WebSession {
  return {
    id,
    connected_at: "2026-07-09T09:00:00Z",
    last_seen: "2026-07-09T10:00:00Z",
    actor_kind: "web",
    actor_id: id,
    actor_display: null,
    safety_mode: safetyMode,
  };
}

const faults: FaultRecord[] = [
  {
    id: "f-1",
    app: "shop",
    kind: "container_crashed",
    timestamp: "2026-07-09T10:00:00Z",
    description: "exited 137",
  },
  {
    id: "f-2",
    app: "blog",
    kind: "healthcheck_failed",
    timestamp: "2026-07-09T10:01:00Z",
    description: "unreachable",
  },
];

const heldVolume: HeldVolume = {
  id: "h-1",
  app: "shop",
  volume_name: "data",
  display_name: "shop/data",
  reason: "app-script update removed the volume",
  held_at: "2026-07-09T09:30:00Z",
};

// The harness sets webSessionId to "test-web-session"; sessions with that id
// belong to us and must not count towards peer elevation.
const clients: ConnectedClients = {
  web: [webSession("test-web-session", "read"), webSession("peer-1", "read")],
  shells: [
    {
      session_id: "sh-1",
      app: "shop",
      name: "db",
      opened_at: "2026-07-09T09:45:00Z",
    },
  ],
  forwards: [],
  actors: [],
};

describe("Navbar", () => {
  it("renders nav links and hostname, and sets the tab title", async () => {
    renderWithSession(<Navbar />, {
      fixtures: {
        "/server/status": { hostname: "sprout.example", version: "0.4.1" },
        "/connected-clients/list": clients,
      },
    });
    expect(await screen.findByText("sprout.example")).toBeTruthy();
    expect(document.title).toBe("sprout.example · Seedling");

    const links: Record<string, string> = {
      "Authorised OI keys": "/keys",
      "Container registry allowlist": "/registries",
      "Container images": "/images",
      Services: "/services",
      "Site ingresses": "/ingresses",
      "TLS certificates": "/certificates",
      Volumes: "/volumes",
      Backups: "/backups",
      Templates: "/templates",
    };
    for (const [name, href] of Object.entries(links)) {
      expect(screen.getByRole("link", { name }).getAttribute("href")).toBe(href);
    }
  });

  it("shows no fault chip and a default title when everything is quiet", async () => {
    renderWithSession(<Navbar />, {
      fixtures: { "/server/status": { hostname: "", version: "0.4.1" } },
    });
    expect(await screen.findByText("Seedling")).toBeTruthy();
    expect(document.title).toBe("Seedling");
    expect(screen.queryByText(/fault/)).toBeNull();
  });

  it("shows the fault chip linking to /faults when faults are active", async () => {
    renderWithSession(<Navbar />, { fixtures: { "/faults/list": faults } });
    const chip = await screen.findByText("2 faults");
    expect(chip.closest("a")?.getAttribute("href")).toBe("/faults");
  });

  it("counts web, shell, and forward sessions in the sessions badge", async () => {
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": clients },
    });
    // 2 web + 1 shell + 0 forwards
    expect(await screen.findByText("3")).toBeTruthy();
  });

  // w[verify sessions.safety-mode]
  it("flags peer sessions in elevated modes but ignores our own session", async () => {
    const elevated: ConnectedClients = {
      ...clients,
      web: [
        webSession("test-web-session", "dangerous"),
        webSession("peer-1", "dangerous"),
        webSession("peer-2", "write"),
      ],
    };
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": elevated },
    });
    const badge = await screen.findByText("4");
    fireEvent.mouseOver(badge.closest("a")!);
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip.textContent).toContain("4 connected clients");
    expect(tooltip.textContent).toContain("1 in dangerous mode");
    expect(tooltip.textContent).toContain("1 in write mode");
  });

  // w[verify sessions.safety-mode]
  it("does not flag elevation when only our own session is elevated", async () => {
    const ownOnly: ConnectedClients = {
      web: [webSession("test-web-session", "dangerous")],
      shells: [],
      forwards: [],
      actors: [],
    };
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": ownOnly },
    });
    const badge = await screen.findByText("1");
    fireEvent.mouseOver(badge.closest("a")!);
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip.textContent).toBe("1 connected client");
  });

  // w[verify routes.volumes.held-count]
  it("shows a held-volume count badge on the volumes link", async () => {
    renderWithSession(<Navbar />, {
      fixtures: { "/volumes/held/list": [heldVolume] },
    });
    const volumesLink = await screen.findByRole("link", {
      name: /1 held volume pending review/,
    });
    expect(volumesLink.getAttribute("href")).toBe("/volumes");
    expect(volumesLink.textContent).toContain("1");
  });
});
