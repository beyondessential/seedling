import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { FaultRecord } from "../lib/types";
import Faults from "./Faults";

const fault: FaultRecord = {
  id: "f-1",
  app: "shop",
  kind: "container_crashed",
  resource_type: "container",
  resource_name: "web",
  instance_id: "0123456789abcdef",
  timestamp: "2026-07-09T10:00:00Z",
  description: "container exited with status 137",
};

describe("Faults", () => {
  it("renders the empty state", async () => {
    renderWithSession(<Faults />, { fixtures: { "/faults/list": [] } });
    expect(await screen.findByText("No active faults.")).toBeTruthy();
  });

  it("renders fault rows with app link and details", async () => {
    renderWithSession(<Faults />, { fixtures: { "/faults/list": [fault] } });
    const link = await screen.findByRole("link", { name: "shop" });
    expect(link.getAttribute("href")).toBe("/apps/shop");
    expect(screen.getByText("container_crashed")).toBeTruthy();
    expect(screen.getByText(/container exited with status 137/)).toBeTruthy();
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Faults />, {
      fixtures: {
        "/faults/list": { ok: false, error: { code: "internal", message: "db exploded" } },
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });
});
