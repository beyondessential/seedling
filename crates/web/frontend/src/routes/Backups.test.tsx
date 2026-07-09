import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Mock } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  AppSummary,
  BackupApp,
  BackupStrategy,
  ExportedVolume,
  SiteVolume,
} from "../lib/types";
import Backups from "./Backups";

const strategy: BackupStrategy = {
  name: "nightly",
  via: "restic",
  schedule: "every day",
  volumes: ["_site/data", "shop/uploads"],
  last_fired_at: null,
  next_fire_at: "2026-07-10T02:00:00Z",
};
const backupApp: BackupApp = { app: "restic" };
const apps: AppSummary[] = [
  { name: "restic", status: "running" },
  { name: "shop", status: "running" },
];
const siteVols: SiteVolume[] = [
  { name: "data", kind: "managed", created_at: "2026-07-01T10:00:00Z" },
];
const exportedVols: ExportedVolume[] = [
  { app: "shop", volume_name: "uploads" },
];

const populated = {
  "/backups/strategies/list": [strategy],
  "/backups/apps/list": [backupApp],
  "/apps/list": apps,
  "/volumes/site/list": siteVols,
  "/volumes/exported/list": exportedVols,
};

const empty = {
  "/backups/strategies/list": [],
  "/backups/apps/list": [],
  "/apps/list": [],
  "/volumes/site/list": [],
  "/volumes/exported/list": [],
};

function iconButton(scope: HTMLElement, label: string): HTMLButtonElement {
  const span = within(scope).getByLabelText(label);
  const btn = span.querySelector("button");
  if (!btn) throw new Error(`no button under tooltip label "${label}"`);
  return btn;
}

function row(text: string): HTMLElement {
  const tr = screen.getByText(text).closest("tr");
  if (!tr) throw new Error(`no table row containing "${text}"`);
  return tr;
}

function selectByLabel(scope: HTMLElement, label: string): HTMLElement {
  for (const l of Array.from(scope.querySelectorAll("label"))) {
    if (l.textContent !== label) continue;
    const combo = l.parentElement?.querySelector<HTMLElement>('[role="combobox"]');
    if (combo) return combo;
  }
  throw new Error(`no select labelled "${label}"`);
}

function callsTo(request: Mock, method: string): unknown[] {
  return request.mock.calls.filter((c) => c[0] === method).map((c) => c[1]);
}

describe("Backups", () => {
  // w[verify routes.backups]
  it("renders strategies and backup apps when populated", async () => {
    renderWithSession(<Backups />, { fixtures: populated });

    expect(await screen.findByText("nightly")).toBeTruthy();
    const strat = row("nightly");
    expect(within(strat).getByText("restic")).toBeTruthy();
    expect(within(strat).getByText("every day")).toBeTruthy();
    expect(within(strat).getByText("never")).toBeTruthy();
    expect(within(strat).getByText("_site/data")).toBeTruthy();
    expect(within(strat).getByText("shop/uploads")).toBeTruthy();

    const appLink = screen.getByRole("link", { name: "restic" });
    expect(appLink.getAttribute("href")).toBe("/apps/restic");
  });

  it("renders empty states and disables strategy creation without a backup app", async () => {
    renderWithSession(<Backups />, { fixtures: empty });
    expect(
      await screen.findByText(/No backup strategies\.\s*Register a backup app first\./),
    ).toBeTruthy();
    expect(screen.getByText(/No backup apps registered/)).toBeTruthy();
    const newBtn = screen.getByRole("button", { name: "New" }) as HTMLButtonElement;
    expect(newBtn.disabled).toBe(true);
  });

  it("shows an error alert when the strategy query fails", async () => {
    renderWithSession(<Backups />, {
      fixtures: {
        ...empty,
        "/backups/strategies/list": {
          ok: false,
          error: { code: "internal", message: "scheduler offline" },
        },
      },
    });
    expect(await screen.findByText(/\[internal\] scheduler offline/)).toBeTruthy();
  });

  it("runs a backup now and reports the queued operations", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: {
        ...populated,
        "/backups/run": [{ volume: "_site/data", operation_id: "op-42" }],
      },
      safetyMode: "write",
    });
    await screen.findByText("nightly");
    fireEvent.click(iconButton(row("nightly"), "Run backup now"));

    expect(await screen.findByText(/Backup triggered for/)).toBeTruthy();
    expect(screen.getByText(/_site\/data → op-42/)).toBeTruthy();
    expect(callsTo(request, "/backups/run")).toEqual([{ strategy: "nightly" }]);
  });

  it("deletes a strategy from its row", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("nightly");
    fireEvent.click(iconButton(row("nightly"), "Delete strategy"));
    await waitFor(() =>
      expect(callsTo(request, "/backups/strategies/delete")).toEqual([{ name: "nightly" }]),
    );
  });

  it("deregisters a backup app from its row", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: populated,
      safetyMode: "write",
    });
    // "restic" also appears in the strategy row's Via column, so anchor on
    // the backup-apps table's link.
    const link = await screen.findByRole("link", { name: "restic" });
    const appRow = link.closest("tr");
    if (!appRow) throw new Error("no backup app row");
    fireEvent.click(iconButton(appRow, "Deregister"));
    await waitFor(() =>
      expect(callsTo(request, "/backups/apps/deregister")).toEqual([{ app: "restic" }]),
    );
  });

  // w[verify routes.backups]
  it("registers a backup app through the dialog", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("nightly");
    fireEvent.click(screen.getByRole("button", { name: "Register" }));

    const dialog = await screen.findByRole("dialog");
    // Defaults to the first app from /apps/list.
    fireEvent.click(within(dialog).getByRole("button", { name: "Register" }));
    await waitFor(() =>
      expect(callsTo(request, "/backups/apps/register")).toEqual([{ app: "restic" }]),
    );
  });

  // w[verify routes.backups]
  it("creates a strategy with name, app, schedule, and volumes", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("nightly");
    fireEvent.click(screen.getByRole("button", { name: "New" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/^Name/), {
      target: { value: "weekly" },
    });
    const create = within(dialog).getByRole("button", { name: "Create" }) as HTMLButtonElement;
    // No volumes selected yet.
    expect(create.disabled).toBe(true);

    fireEvent.mouseDown(selectByLabel(dialog, "Volumes"));
    fireEvent.click(await screen.findByRole("option", { name: "_site/data" }));
    fireEvent.click(create);

    await waitFor(() =>
      expect(callsTo(request, "/backups/strategies/create")).toEqual([
        {
          name: "weekly",
          via: "restic",
          schedule: "every day",
          volumes: ["_site/data"],
        },
      ]),
    );
  });

  // w[verify routes.backups]
  it("lists snapshots for a strategy and restores one", async () => {
    const { request } = renderWithSession(<Backups />, {
      fixtures: {
        ...populated,
        "/backups/snapshots/list": [
          { id: "snap-1", time: "2026-07-08T02:00:00Z" },
        ],
        "/backups/restore": { site_volume: "data-restored" },
      },
      safetyMode: "write",
    });
    await screen.findByText("nightly");
    fireEvent.click(iconButton(row("nightly"), "List snapshots / restore"));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Snapshots — nightly/)).toBeTruthy();
    // Defaults to the strategy's first volume.
    expect(await within(dialog).findByText("snap-1")).toBeTruthy();
    expect(callsTo(request, "/backups/snapshots/list")).toEqual([
      { strategy: "nightly", volume: "_site/data" },
    ]);

    fireEvent.click(iconButton(dialog, 'Restore snapshot "snap-1"'));
    expect(await within(dialog).findByText(/Restored to site volume/)).toBeTruthy();
    expect(within(dialog).getByText("data-restored")).toBeTruthy();
    expect(callsTo(request, "/backups/restore")).toEqual([
      { strategy: "nightly", volume: "_site/data", snapshot: "snap-1" },
    ]);
  });

  it("shows the snapshots empty state", async () => {
    renderWithSession(<Backups />, {
      fixtures: { ...populated, "/backups/snapshots/list": [] },
    });
    await screen.findByText("nightly");
    fireEvent.click(iconButton(row("nightly"), "List snapshots / restore"));
    expect(await screen.findByText("No snapshots found.")).toBeTruthy();
  });
});
