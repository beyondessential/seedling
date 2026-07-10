import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Mock } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  AppVolume,
  DeclaredExternalVolume,
  ExportedVolume,
  ExternalMapping,
  HeldVolume,
  SiteVolume,
} from "../lib/types";
import Volumes from "./Volumes";

const siteManaged: SiteVolume = {
  name: "data",
  kind: "managed",
  created_at: "2026-07-01T10:00:00Z",
};
const siteBind: SiteVolume = {
  name: "media",
  kind: "bind",
  created_at: "2026-07-02T10:00:00Z",
  host_path: "/srv/media",
};
const siteSnap: SiteVolume = {
  name: "data-snap",
  kind: "snapshot",
  created_at: "2026-07-03T10:00:00Z",
  source: "_site/data",
};

const exported: ExportedVolume = {
  app: "shop",
  volume_name: "uploads",
  description: "User uploads",
};
const appVol: AppVolume = {
  app: "shop",
  volume_name: "uploads",
  exported: true,
  description: "User uploads",
};

const declaredMapped: DeclaredExternalVolume = { app: "shop", name: "shared" };
const declaredUnmapped: DeclaredExternalVolume = { app: "blog", name: "assets" };
const mapping: ExternalMapping = {
  app: "shop",
  external_name: "shared",
  read_only: true,
  target: { kind: "site", name: "data" },
};

const held: HeldVolume = {
  id: "h-1",
  app: "shop",
  volume_name: "olddata",
  display_name: "olddata",
  reason: "deleted by operator",
  held_at: "2026-07-05T10:00:00Z",
};

const populated = {
  "/volumes/site/list": [siteManaged, siteBind, siteSnap],
  "/volumes/app/list": [appVol],
  "/volumes/exported/list": [exported],
  "/volumes/external/list": [mapping],
  "/volumes/external/declared": [declaredMapped, declaredUnmapped],
  "/volumes/held/list": [held],
};

const empty = {
  "/volumes/site/list": [],
  "/volumes/app/list": [],
  "/volumes/exported/list": [],
  "/volumes/external/list": [],
  "/volumes/external/declared": [],
  "/volumes/held/list": [],
};

/** Find the actual <button> under an ActionButton's tooltip label (the
 *  aria-label lands on the wrapping span, not the button element). */
function iconButton(scope: HTMLElement, label: string): HTMLButtonElement {
  const holder = within(scope).getByLabelText(label);
  const btn =
    holder.tagName === "BUTTON"
      ? (holder as HTMLButtonElement)
      : holder.querySelector("button");
  if (!btn) throw new Error(`no button labelled "${label}"`);
  return btn;
}

function row(text: string): HTMLElement {
  const tr = screen.getByText(text).closest("tr");
  if (!tr) throw new Error(`no table row containing "${text}"`);
  return tr;
}

/** Find a MUI Select's combobox by its InputLabel text. The bare
 *  FormControl+InputLabel combos in these dialogs have no id wiring, so the
 *  combobox carries no accessible name to query by. */
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

describe("Volumes", () => {
  it("shows spinners while queries are in flight", async () => {
    renderWithSession(<Volumes />, { fixtures: empty });
    expect(screen.getAllByRole("progressbar").length).toBeGreaterThan(0);
    await screen.findByText("No site volumes.");
  });

  // w[verify routes.volumes]
  it("renders all sections when populated", async () => {
    renderWithSession(<Volumes />, { fixtures: populated });

    // Site volumes with kind chips and info column.
    expect(await screen.findByText("data")).toBeTruthy();
    expect(screen.getByText("managed")).toBeTruthy();
    expect(screen.getByText("/srv/media")).toBeTruthy();
    expect(within(row("data-snap")).getByText("_site/data")).toBeTruthy();

    // App exports, with app link.
    const shopLinks = screen.getAllByRole("link", { name: "shop" });
    expect(shopLinks[0].getAttribute("href")).toBe("/apps/shop");
    expect(screen.getByText("uploads")).toBeTruthy();
    expect(screen.getByText("User uploads")).toBeTruthy();

    // External volume requests: mapped row shows the target and ro chip,
    // undeclared row shows "unmapped".
    const mapped = row("shared");
    expect(within(mapped).getByText("_site/data")).toBeTruthy();
    expect(within(mapped).getByText("ro")).toBeTruthy();
    expect(within(row("assets")).getByText("unmapped")).toBeTruthy();

    // Held volumes section appears when there are held volumes.
    expect(screen.getByText("Held Volumes")).toBeTruthy();
    expect(screen.getByText("deleted by operator")).toBeTruthy();
  });

  it("renders empty states and hides the held section", async () => {
    renderWithSession(<Volumes />, { fixtures: empty });
    expect(await screen.findByText("No site volumes.")).toBeTruthy();
    expect(screen.getByText("No exported volumes.")).toBeTruthy();
    expect(
      screen.getByText("No external volume requests across registered apps."),
    ).toBeTruthy();
    expect(screen.queryByText("Held Volumes")).toBeNull();
    // Nothing to open a shell over.
    const openShell = screen.getByRole("button", { name: "Open shell…" });
    expect((openShell as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows an error alert when a section query fails", async () => {
    renderWithSession(<Volumes />, {
      fixtures: {
        ...empty,
        "/volumes/site/list": {
          ok: false,
          error: { code: "internal", message: "db exploded" },
        },
      },
    });
    expect(await screen.findByText(/\[internal\] db exploded/)).toBeTruthy();
  });

  it("keeps write and dangerous actions disabled in read mode", async () => {
    renderWithSession(<Volumes />, { fixtures: populated });
    await screen.findByText("data");
    expect((screen.getByRole("button", { name: "New" }) as HTMLButtonElement).disabled).toBe(true);
    const managed = row("data");
    expect(iconButton(managed, "Snapshot").disabled).toBe(true);
    expect(iconButton(managed, "Delete").disabled).toBe(true);
    expect(iconButton(managed, "Open shell (read-only)").disabled).toBe(false);
  });

  // w[verify volumes.shell-ui]
  // w[verify volumes.shell-ui.read-only]
  it("opens a read-only volume shell from a site row in read mode", async () => {
    const { openVolumeShell } = renderWithSession(<Volumes />, { fixtures: populated });
    await screen.findByText("data");
    fireEvent.click(iconButton(row("data"), "Open shell (read-only)"));
    expect(openVolumeShell).toHaveBeenCalledWith(
      [{ kind: "site", name: "data" }],
      "data",
      { readOnly: true },
    );
  });

  // w[verify volumes.shell-ui.read-only]
  it("opens a read-write volume shell in write mode", async () => {
    const { openVolumeShell } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("uploads");
    fireEvent.click(iconButton(row("uploads"), "Open shell"));
    expect(openVolumeShell).toHaveBeenCalledWith(
      [{ kind: "app", app: "shop", volume: "uploads" }],
      "shop/uploads",
      { readOnly: false },
    );
  });

  // w[verify routes.volumes.delete-confirm]
  it("confirms managed site volume deletion before issuing the request", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("data");
    fireEvent.click(iconButton(row("data"), "Delete"));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/moved into the held-volumes list/)).toBeTruthy();
    expect(callsTo(request, "/volumes/site/delete")).toHaveLength(0);

    fireEvent.click(within(dialog).getByRole("button", { name: "Move to held" }));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/site/delete")).toEqual([{ name: "data" }]),
    );
  });

  // w[verify routes.volumes.delete-confirm]
  it("keeps the delete dialog open and shows the error when deletion fails", async () => {
    renderWithSession(<Volumes />, {
      fixtures: {
        ...populated,
        "/volumes/site/delete": {
          ok: false,
          error: { code: "internal", message: "disk on fire" },
        },
      },
      safetyMode: "dangerous",
    });
    await screen.findByText("data");
    fireEvent.click(iconButton(row("data"), "Delete"));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Move to held" }));
    expect(await within(dialog).findByText(/disk on fire/)).toBeTruthy();
  });

  // w[verify routes.volumes.delete-confirm]
  it("states kind-specific consequences for snapshot and bind deletion", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("data-snap");

    fireEvent.click(iconButton(row("data-snap"), "Delete"));
    let dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/permanently deleted and cannot be\s*recovered/)).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Delete snapshot" })).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(iconButton(row("media"), "Delete"));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/bind-mount reference/)).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Delete reference" })).toBeTruthy();
    expect(callsTo(request, "/volumes/site/delete")).toHaveLength(0);
  });

  // w[verify routes.volumes.delete-confirm]
  it("confirms held volume deletion and issues the delete", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "dangerous",
    });
    await screen.findByText("Held Volumes");
    fireEvent.click(iconButton(row("olddata"), "Confirm delete"));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/erase the volume's data irreversibly/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete permanently" }));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/held/delete")).toEqual([{ id: "h-1" }]),
    );
  });

  it("restores a held volume under a new name, refusing collisions", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("Held Volumes");
    fireEvent.click(iconButton(row("olddata"), "Restore as site volume"));

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/New site volume name/) as HTMLInputElement;
    expect(input.value).toBe("olddata");

    // Colliding with an existing site volume blocks submission.
    fireEvent.change(input, { target: { value: "data" } });
    expect(within(dialog).getByText(/already exists/)).toBeTruthy();
    const restore = within(dialog).getByRole("button", { name: "Restore" }) as HTMLButtonElement;
    expect(restore.disabled).toBe(true);

    fireEvent.change(input, { target: { value: "newdata" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Restore" }));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/held/restore")).toEqual([
        { id: "h-1", target_name: "newdata" },
      ]),
    );
  });

  it("unmaps an external volume from its row", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("shared");
    fireEvent.click(iconButton(row("shared"), "Unmap"));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/external/unmap")).toEqual([
        { app: "shop", external_name: "shared" },
      ]),
    );
  });

  it("maps an unmapped request with the app and name prefilled", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("assets");
    fireEvent.click(within(row("assets")).getByRole("button", { name: "Map" }));

    const dialog = await screen.findByRole("dialog");
    const fixed = within(dialog).getByLabelText(/App \/ External volume/) as HTMLInputElement;
    expect(fixed.value).toBe("blog / assets");
    expect(fixed.disabled).toBe(true);

    fireEvent.mouseDown(selectByLabel(dialog, "Site volume"));
    fireEvent.click(await screen.findByRole("option", { name: /^data(?!-snap)/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Map" }));

    await waitFor(() =>
      expect(callsTo(request, "/volumes/external/map")).toEqual([
        {
          app: "blog",
          external_name: "assets",
          target: { kind: "site", name: "data" },
          read_only: false,
        },
      ]),
    );
  });

  it("snapshots a site volume via the row action", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("data");
    fireEvent.click(iconButton(row("data"), "Snapshot"));

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/Snapshot name/) as HTMLInputElement;
    expect(input.value).toMatch(/^_site-data-\d{8}-\d{6}$/);

    fireEvent.click(within(dialog).getByRole("button", { name: "Snapshot" }));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/site/snapshot")).toEqual([
        { name: input.value, source: "_site/data" },
      ]),
    );
  });

  it("promotes a snapshot volume to a fresh managed volume", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("data-snap");
    // Only the snapshot-kind row offers promotion.
    expect(within(row("data")).queryByLabelText("Promote to read-write volume")).toBeNull();
    fireEvent.click(iconButton(row("data-snap"), "Promote to read-write volume"));

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/New volume name/) as HTMLInputElement;
    expect(input.value).toBe("data-snap-promoted");

    fireEvent.click(within(dialog).getByRole("button", { name: "Promote" }));
    await waitFor(() =>
      expect(callsTo(request, "/volumes/site/promote")).toEqual([
        { source: "data-snap", name: "data-snap-promoted" },
      ]),
    );
  });

  // w[verify routes.volumes]
  it("creates a managed site volume through the New dialog", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("data");
    fireEvent.click(screen.getByRole("button", { name: "New" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/^Name/), {
      target: { value: "fresh" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() =>
      expect(callsTo(request, "/volumes/site/create")).toEqual([
        { name: "fresh", kind: "managed" },
      ]),
    );
  });

  it("creates a bind site volume with a host path", async () => {
    const { request } = renderWithSession(<Volumes />, {
      fixtures: populated,
      safetyMode: "write",
    });
    await screen.findByText("data");
    fireEvent.click(screen.getByRole("button", { name: "New" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/^Name/), {
      target: { value: "binder" },
    });
    fireEvent.click(within(dialog).getByLabelText("Bind mount"));
    const create = within(dialog).getByRole("button", { name: "Create" }) as HTMLButtonElement;
    // Bind kind requires a host path.
    expect(create.disabled).toBe(true);
    fireEvent.change(within(dialog).getByLabelText(/Host path/), {
      target: { value: "/srv/binder" },
    });
    fireEvent.click(create);

    await waitFor(() =>
      expect(callsTo(request, "/volumes/site/create")).toEqual([
        { name: "binder", kind: "bind", host_path: "/srv/binder" },
      ]),
    );
  });

  // w[verify volumes.shell-ui.read-only]
  it("opens a multi-volume shell over the selected volumes", async () => {
    const { openVolumeShell } = renderWithSession(<Volumes />, { fixtures: populated });
    await screen.findByText("data");
    fireEvent.click(screen.getByRole("button", { name: "Open shell…" }));

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByText("data"));
    fireEvent.click(within(dialog).getByText("shop/uploads"));

    const open = within(dialog).getByRole("button", {
      name: "Open shell (read-only) (2)",
    });
    fireEvent.click(open);

    expect(openVolumeShell).toHaveBeenCalledWith(
      [
        { kind: "site", name: "data" },
        { kind: "app", app: "shop", volume: "uploads" },
      ],
      "2 volumes",
      { readOnly: true },
    );
  });
});
