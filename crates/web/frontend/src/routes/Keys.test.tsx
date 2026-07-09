import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { AuthorizedKey } from "../lib/types";
import Keys from "./Keys";

const key: AuthorizedKey = {
  fingerprint: "2ed3cbdf3649b7c24d6a04b07dd052c1eca9648b28817b32b00db82df40c093e",
  label: "felix",
  // Unix timestamp in seconds: 2026-07-06T…Z. As milliseconds this would be
  // ~22 January 1970, which is exactly the bug this test guards against.
  added_at: 1783520875,
};

describe("Keys", () => {
  // w[verify routes.keys]
  it("warns when no keys are authorised", async () => {
    renderWithSession(<Keys />, { fixtures: { "/keys/list": [] } });
    expect(await screen.findByText(/No authorised keys/)).toBeTruthy();
  });

  // w[verify routes.keys]
  it("renders key rows with label and fingerprint", async () => {
    renderWithSession(<Keys />, { fixtures: { "/keys/list": [key] } });
    expect(await screen.findByText("felix")).toBeTruthy();
    expect(screen.getByText(key.fingerprint)).toBeTruthy();
  });

  it("renders added_at as a Unix timestamp in seconds, not milliseconds", async () => {
    renderWithSession(<Keys />, { fixtures: { "/keys/list": [key] } });

    const row = (await screen.findByText(key.label)).closest("tr");
    expect(row).not.toBeNull();
    const cellText = row!.textContent ?? "";

    // The correct rendering multiplies seconds by 1000; the historical bug
    // passed the seconds value straight to `new Date()`, landing in 1970.
    const expected = new Date(key.added_at * 1000).toLocaleString();
    const buggy = new Date(key.added_at).toLocaleString();

    expect(cellText).toContain(expected);
    expect(cellText).not.toContain(buggy);
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Keys />, {
      fixtures: {
        "/keys/list": { ok: false, error: { code: "internal", message: "db exploded" } },
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });

  // w[verify routes.keys]
  it("authorises a key via /keys/authorise in dangerous mode", async () => {
    const fp = "a".repeat(64);
    const { request } = renderWithSession(<Keys />, {
      fixtures: { "/keys/list": [key] },
      safetyMode: "dangerous",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Authorise key" }));
    fireEvent.change(screen.getByLabelText("Fingerprint"), { target: { value: fp } });
    fireEvent.change(screen.getByLabelText("Label"), { target: { value: "ci-runner" } });
    fireEvent.click(screen.getByRole("button", { name: "Authorise" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/keys/authorise",
        { fingerprint: fp, label: "ci-runner" },
      ]),
    );
  });

  // w[verify routes.keys]
  it("revokes a key via /keys/revoke after confirmation", async () => {
    const { request } = renderWithSession(<Keys />, {
      fixtures: { "/keys/list": [key] },
      safetyMode: "dangerous",
    });
    const row = (await screen.findByText("felix")).closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    fireEvent.click(await screen.findByRole("button", { name: "Revoke" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/keys/revoke",
        { fingerprint: key.fingerprint },
      ]),
    );
  });

  it("keeps the authorise and revoke buttons disabled in read mode", async () => {
    renderWithSession(<Keys />, { fixtures: { "/keys/list": [key] } });
    const row = (await screen.findByText("felix")).closest("tr")!;
    expect(within(row).getByRole("button")).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Authorise key" })).toHaveProperty(
      "disabled",
      true,
    );
  });
});
