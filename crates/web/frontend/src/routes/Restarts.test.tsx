import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { RestartRecord } from "../lib/types";
import Restarts from "./Restarts";

const recovery: RestartRecord = {
  id: 2,
  app: "shop",
  instance_id: "0123456789abcdef0123",
  resource_type: "deployment",
  resource_name: "web",
  generation: 4,
  timestamp: "2026-07-09T10:00:00Z",
  cause: "recovery",
  exit_code: 137,
  exit_kind: "exited",
};

const deliberate: RestartRecord = {
  id: 1,
  app: "shop",
  instance_id: "0123456789abcdef0123",
  resource_type: "deployment",
  resource_name: "web",
  generation: 4,
  timestamp: "2026-07-09T09:00:00Z",
  cause: "deliberate",
  exit_code: null,
  exit_kind: null,
};

const settings = { threshold: 5, window_secs: 1800 };

describe("Restarts", () => {
  it("renders the empty state", async () => {
    renderWithSession(<Restarts />, {
      fixtures: { "/restarts/list": [], "/restarts/settings/get": settings },
    });
    expect(await screen.findByText("No restarts recorded.")).toBeTruthy();
  });

  // w[verify routes.restarts]
  it("lists records with their exit status and app link", async () => {
    renderWithSession(<Restarts />, {
      fixtures: {
        "/restarts/list": [recovery, deliberate],
        "/restarts/settings/get": settings,
      },
    });
    const link = await screen.findAllByRole("link", { name: "shop" });
    expect(link[0].getAttribute("href")).toBe("/apps/shop");
    expect(screen.getAllByText("deployment/web").length).toBe(2);
    expect(screen.getByText("exit 137")).toBeTruthy();
    // An unrecorded exit says so rather than showing a fabricated code.
    expect(screen.getByText("unknown")).toBeTruthy();
  });

  // w[verify routes.restarts]
  it("distinguishes deliberate restarts from recovery ones", async () => {
    renderWithSession(<Restarts />, {
      fixtures: {
        "/restarts/list": [recovery, deliberate],
        "/restarts/settings/get": settings,
      },
    });
    expect(await screen.findByText("recovery")).toBeTruthy();
    expect(screen.getByText("deliberate")).toBeTruthy();
  });

  // w[verify routes.restarts]
  it("shows the crash-loop threshold and window", async () => {
    renderWithSession(<Restarts />, {
      fixtures: { "/restarts/list": [], "/restarts/settings/get": settings },
    });
    expect(
      await screen.findByText(
        /5 recovery restarts within 30 minutes/,
      ),
    ).toBeTruthy();
  });

  // w[verify routes.restarts]
  it("sends the window in seconds when the operator saves it in minutes", async () => {
    const setSettings = vi.fn(() => settings);
    renderWithSession(<Restarts />, {
      safetyMode: "write",
      fixtures: {
        "/restarts/list": [],
        "/restarts/settings/get": settings,
        "/restarts/settings/set": setSettings,
      },
    });

    fireEvent.change(await screen.findByLabelText("Threshold"), {
      target: { value: "3" },
    });
    fireEvent.change(screen.getByLabelText("Window"), {
      target: { value: "10" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(setSettings).toHaveBeenCalledWith({
        threshold: 3,
        window_secs: 600,
      }),
    );
  });

  // w[verify routes.restarts]
  it("filters by app", async () => {
    const list = vi.fn(() => []);
    renderWithSession(<Restarts />, {
      fixtures: {
        "/restarts/list": list,
        "/restarts/settings/get": settings,
        "/apps/list": [{ name: "shop", status: "running" }],
      },
    });

    fireEvent.mouseDown(await screen.findByRole("combobox", { name: "App" }));
    const options = await screen.findByRole("listbox");
    fireEvent.click(within(options).getByRole("option", { name: "shop" }));

    await waitFor(() => expect(list).toHaveBeenCalledWith({ app: "shop" }));
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Restarts />, {
      fixtures: {
        "/restarts/list": {
          ok: false,
          error: { code: "internal", message: "db exploded" },
        },
        "/restarts/settings/get": settings,
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });
});
