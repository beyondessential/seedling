import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import { PromoteSnapshotDialog } from "./PromoteSnapshotDialog";

describe("PromoteSnapshotDialog", () => {
  it("strips the timestamp suffix for the suggested name and submits", async () => {
    const onSuccess = vi.fn();
    const onClose = vi.fn();
    const { request } = renderWithSession(
      <PromoteSnapshotDialog
        source="data-20260421-134530"
        onClose={onClose}
        onSuccess={onSuccess}
      />,
      // The dialog treats a null result as failure, so resolve with a value.
      { safetyMode: "write", fixtures: { "/volumes/site/promote": {} } },
    );

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/New volume name/) as HTMLInputElement;
    expect(input.value).toBe("data-promoted");

    fireEvent.click(within(dialog).getByRole("button", { name: "Promote" }));
    await waitFor(() =>
      expect(
        request.mock.calls.filter((c) => c[0] === "/volumes/site/promote").map((c) => c[1]),
      ).toEqual([{ source: "data-20260421-134530", name: "data-promoted" }]),
    );
    expect(onSuccess).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("strips a -snapshot suffix and disables submission on a blank name", async () => {
    const { request } = renderWithSession(
      <PromoteSnapshotDialog source="db-snapshot" onClose={vi.fn()} onSuccess={vi.fn()} />,
      { safetyMode: "write" },
    );
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText(/New volume name/) as HTMLInputElement;
    expect(input.value).toBe("db-promoted");

    fireEvent.change(input, { target: { value: "" } });
    const submit = within(dialog).getByRole("button", { name: "Promote" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(request.mock.calls.some((c) => c[0] === "/volumes/site/promote")).toBe(false);
  });
});
