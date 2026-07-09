import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import { SnapshotVolumeDialog } from "./SnapshotVolumeDialog";

describe("SnapshotVolumeDialog", () => {
  it("suggests a sanitised timestamped name and submits the snapshot", async () => {
    const onSuccess = vi.fn();
    const onClose = vi.fn();
    const { request } = renderWithSession(
      <SnapshotVolumeDialog
        source="shop/uploads"
        sourceLabel="shop/uploads"
        onClose={onClose}
        onSuccess={onSuccess}
      />,
      // The dialog treats a null result as failure, so resolve with a value.
      { safetyMode: "write", fixtures: { "/volumes/site/snapshot": {} } },
    );

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/Snapshot name/) as HTMLInputElement;
    // The `/` in the source is sanitised to `-`.
    expect(input.value).toMatch(/^shop-uploads-\d{8}-\d{6}$/);

    fireEvent.change(input, { target: { value: "uploads-before-upgrade" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Snapshot" }));

    await waitFor(() =>
      expect(
        request.mock.calls.filter((c) => c[0] === "/volumes/site/snapshot").map((c) => c[1]),
      ).toEqual([{ name: "uploads-before-upgrade", source: "shop/uploads" }]),
    );
    expect(onSuccess).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("disables submission while the name is blank", async () => {
    const { request } = renderWithSession(
      <SnapshotVolumeDialog
        source="_site/data"
        sourceLabel="data"
        onClose={vi.fn()}
        onSuccess={vi.fn()}
      />,
      { safetyMode: "write" },
    );
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/Snapshot name/) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "   " } });

    const submit = within(dialog).getByRole("button", { name: "Snapshot" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(request.mock.calls.some((c) => c[0] === "/volumes/site/snapshot")).toBe(false);
  });
});
