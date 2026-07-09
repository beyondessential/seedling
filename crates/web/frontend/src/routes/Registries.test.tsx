import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import Registries from "./Registries";

const populated = { registries: ["docker.io", "registry.example.com:5000"] };

describe("Registries", () => {
  // w[verify routes.registries]
  it("warns when the allowlist is empty", async () => {
    renderWithSession(<Registries />, {
      fixtures: { "/registries/list": { registries: [] } },
    });
    expect(await screen.findByText(/The allowlist is empty/)).toBeTruthy();
  });

  // w[verify routes.registries]
  it("renders registry rows", async () => {
    renderWithSession(<Registries />, {
      fixtures: { "/registries/list": populated },
    });
    expect(await screen.findByText("docker.io")).toBeTruthy();
    expect(screen.getByText("registry.example.com:5000")).toBeTruthy();
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Registries />, {
      fixtures: {
        "/registries/list": {
          ok: false,
          error: { code: "internal", message: "db exploded" },
        },
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });

  // w[verify routes.registries]
  it("adds a registry via /registries/add in write mode", async () => {
    const { request } = renderWithSession(<Registries />, {
      fixtures: { "/registries/list": populated },
      safetyMode: "write",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add registry" }));
    fireEvent.change(screen.getByLabelText("Registry hostname"), {
      target: { value: "ghcr.io" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/registries/add",
        { registry: "ghcr.io" },
      ]),
    );
  });

  // w[verify routes.registries]
  it("removes a registry via /registries/remove after confirmation", async () => {
    const { request } = renderWithSession(<Registries />, {
      fixtures: { "/registries/list": populated },
      safetyMode: "dangerous",
    });
    const row = (await screen.findByText("docker.io")).closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/registries/remove",
        { registry: "docker.io" },
      ]),
    );
  });

  it("keeps mutating buttons disabled in read mode", async () => {
    renderWithSession(<Registries />, {
      fixtures: { "/registries/list": populated },
    });
    const row = (await screen.findByText("docker.io")).closest("tr")!;
    expect(within(row).getByRole("button")).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Add registry" })).toHaveProperty(
      "disabled",
      true,
    );
  });
});
