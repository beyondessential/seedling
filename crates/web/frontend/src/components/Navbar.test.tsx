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
      "Canopy access and reporting": "/canopy",
      "Container images": "/images",
      Services: "/services",
      "Site ingresses": "/ingresses",
      "TLS certificates": "/certificates",
      Volumes: "/volumes",
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

  // spec: UI#connected-clients
  it("counts only open web sessions in the sessions badge", async () => {
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": clients },
    });
    // 2 web sessions; the open shell does not count.
    expect(await screen.findByText("2")).toBeTruthy();
  });

  // spec: UI#guarded-controls
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
    const badge = await screen.findByText("3");
    fireEvent.mouseOver(badge.closest("a")!);
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip.textContent).toContain("3 open web sessions");
    expect(tooltip.textContent).toContain("1 in dangerous mode");
    expect(tooltip.textContent).toContain("1 in write mode");
  });

  // spec: UI#guarded-controls
  it("flags the safety mode control with the highest peer mode, whatever our own mode", async () => {
    const elevated: ConnectedClients = {
      ...clients,
      web: [
        webSession("test-web-session", "read"),
        webSession("peer-1", "write"),
        webSession("peer-2", "dangerous"),
      ],
    };
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": elevated },
    });
    const flag = await screen.findByTestId("peer-mode-flag");
    expect(flag.getAttribute("data-peer-mode")).toBe("dangerous");
    expect(flag.className).toContain("MuiChip-colorError");
  });

  // spec: UI#guarded-controls
  it("flags a write-mode peer in the warning colour and flags nothing for read peers", async () => {
    const { unmount } = renderWithSession(<Navbar />, {
      fixtures: {
        "/connected-clients/list": { ...clients, web: [webSession("peer-1", "write")] },
      },
    });
    const flag = await screen.findByTestId("peer-mode-flag");
    expect(flag.getAttribute("data-peer-mode")).toBe("write");
    expect(flag.className).toContain("MuiChip-colorWarning");
    unmount();
    renderWithSession(<Navbar />, {
      fixtures: { "/connected-clients/list": clients },
    });
    await screen.findByText("2");
    expect(screen.queryByTestId("peer-mode-flag")).toBeNull();
  });

  // spec: UI#guarded-controls
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
    expect(tooltip.textContent).toBe("1 open web session");
    expect(screen.queryByTestId("peer-mode-flag")).toBeNull();
  });

  // spec: UI#operator-capabilities
  it("says so when a count could not be read instead of showing none", async () => {
    renderWithSession(<Navbar />, {
      fixtures: {
        "/faults/list": { ok: false, error: { code: "internal", message: "db gone" } },
      },
    });
    expect(await screen.findByText("counts unavailable")).toBeTruthy();
  });

  // spec: UI#volumes
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
