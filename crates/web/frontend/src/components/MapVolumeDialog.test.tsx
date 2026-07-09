import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";
import { renderWithSession } from "../test/harness";
import type {
  DeclaredExternalVolume,
  ExportedVolume,
  ExternalMapping,
  SiteVolume,
} from "../lib/types";
import { MapVolumeDialog } from "./MapVolumeDialog";

const siteVols: SiteVolume[] = [
  { name: "data", kind: "managed", created_at: "2026-07-01T10:00:00Z" },
];
const exportedVols: ExportedVolume[] = [
  { app: "shop", volume_name: "uploads", description: "User uploads" },
];
const declared: DeclaredExternalVolume[] = [
  { app: "blog", name: "assets" },
];

const fixtures = {
  "/volumes/site/list": siteVols,
  "/volumes/exported/list": exportedVols,
  "/volumes/external/declared": declared,
  // Mutations must resolve with a non-null value: execute() treats null
  // as failure and the dialog skips onSuccess.
  "/volumes/external/map": { mapped: true },
  "/volumes/external/remap": { remapped: true },
};

const emptyFixtures = {
  "/volumes/site/list": [],
  "/volumes/exported/list": [],
  "/volumes/external/declared": [],
};

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

describe("MapVolumeDialog", () => {
  it("maps a declared request to a site volume", async () => {
    const onSuccess = vi.fn();
    const { request } = renderWithSession(
      <MapVolumeDialog open onClose={vi.fn()} onSuccess={onSuccess} />,
      { fixtures, safetyMode: "write" },
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Map External Volume")).toBeTruthy();

    // Pick the app/external-volume request.
    fireEvent.mouseDown(selectByLabel(dialog, "App / External volume"));
    fireEvent.click(await screen.findByRole("option", { name: /blog/ }));

    // Target defaults to site volume; pick one.
    fireEvent.mouseDown(selectByLabel(dialog, "Site volume"));
    fireEvent.click(await screen.findByRole("option", { name: /data/ }));

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
    expect(onSuccess).toHaveBeenCalled();
  });

  it("remaps an existing mapping with app and name fixed", async () => {
    const existing: ExternalMapping = {
      app: "blog",
      external_name: "assets",
      read_only: true,
      target: { kind: "site", name: "old-target" },
    };
    const { request } = renderWithSession(
      <MapVolumeDialog open onClose={vi.fn()} onSuccess={vi.fn()} existing={existing} />,
      { fixtures, safetyMode: "write" },
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Remap External Volume")).toBeTruthy();
    const fixed = within(dialog).getByLabelText(/App \/ External volume/) as HTMLInputElement;
    expect(fixed.value).toBe("blog / assets");
    expect(fixed.disabled).toBe(true);

    fireEvent.mouseDown(selectByLabel(dialog, "Site volume"));
    fireEvent.click(await screen.findByRole("option", { name: /data/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Remap" }));

    await waitFor(() =>
      expect(callsTo(request, "/volumes/external/remap")).toEqual([
        {
          app: "blog",
          external_name: "assets",
          target: { kind: "site", name: "data" },
          read_only: true,
        },
      ]),
    );
  });

  it("falls back to manual entry for an app-volume target and honours read-only", async () => {
    const { request } = renderWithSession(
      <MapVolumeDialog
        open
        onClose={vi.fn()}
        onSuccess={vi.fn()}
        prefill={{ app: "blog", name: "assets" }}
      />,
      { fixtures: emptyFixtures, safetyMode: "write" },
    );
    const dialog = await screen.findByRole("dialog");

    fireEvent.click(within(dialog).getByLabelText("Exported app volume"));
    fireEvent.change(within(dialog).getByLabelText(/Source app/), {
      target: { value: "shop" },
    });
    fireEvent.change(within(dialog).getByLabelText(/Exported volume name/), {
      target: { value: "uploads" },
    });
    fireEvent.click(within(dialog).getByLabelText("Mount read-only"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Map" }));

    await waitFor(() =>
      expect(callsTo(request, "/volumes/external/map")).toEqual([
        {
          app: "blog",
          external_name: "assets",
          target: { kind: "app", app: "shop", volume: "uploads" },
          read_only: true,
        },
      ]),
    );
  });

  it("shows the OI error and does not close when mapping fails", async () => {
    const onSuccess = vi.fn();
    const onClose = vi.fn();
    renderWithSession(
      <MapVolumeDialog
        open
        onClose={onClose}
        onSuccess={onSuccess}
        prefill={{ app: "blog", name: "assets" }}
      />,
      {
        fixtures: {
          ...fixtures,
          "/volumes/external/map": {
            ok: false,
            error: { code: "conflict", message: "already mapped" },
          },
        },
        safetyMode: "write",
      },
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.mouseDown(selectByLabel(dialog, "Site volume"));
    fireEvent.click(await screen.findByRole("option", { name: /data/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Map" }));

    expect(await within(dialog).findByText(/\[conflict\] already mapped/)).toBeTruthy();
    expect(onSuccess).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });
});
