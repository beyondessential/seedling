import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PlanResponse } from "../lib/types";
import { PlanDiff } from "./PlanDiff";

describe("PlanDiff", () => {
  it("shows the empty state when the plan has no changes", () => {
    render(<PlanDiff plan={{}} />);
    expect(screen.getByText("Resource changes (0)")).toBeTruthy();
    expect(screen.getByText("No resource changes.")).toBeTruthy();
    expect(screen.queryByText(/on_change handlers/)).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("renders added, modified, and removed entries with their fields", () => {
    const plan: PlanResponse = {
      diff: [
        { resource_type: "Container", resource_name: "web", change: "added" },
        {
          resource_type: "Container",
          resource_name: "worker",
          change: "modified",
          fields: ["image", "scale"],
        },
        { resource_type: "Volume", resource_name: "cache", change: "removed" },
      ],
    };
    render(<PlanDiff plan={plan} />);
    expect(screen.getByText("Resource changes (3)")).toBeTruthy();
    expect(screen.getByText("added")).toBeTruthy();
    expect(screen.getByText("modified")).toBeTruthy();
    expect(screen.getByText("removed")).toBeTruthy();
    expect(screen.getByText("web")).toBeTruthy();
    expect(screen.getByText("worker")).toBeTruthy();
    expect(screen.getByText("cache")).toBeTruthy();
    expect(screen.getByText("volume")).toBeTruthy();
    expect(screen.getByText("image, scale")).toBeTruthy();
  });

  it("lists on_change handlers that would fire", () => {
    const plan: PlanResponse = {
      diff: [],
      on_change_would_fire: ["migrate", "reload-config"],
    };
    render(<PlanDiff plan={plan} />);
    expect(
      screen.getByText("on_change handlers that would fire (2)"),
    ).toBeTruthy();
    expect(screen.getByText("migrate")).toBeTruthy();
    expect(screen.getByText("reload-config")).toBeTruthy();
  });

  it("surfaces plan errors as alerts", () => {
    const plan: PlanResponse = {
      errors: ["script failed: line 3", "unknown resource kind"],
    };
    render(<PlanDiff plan={plan} />);
    expect(screen.getByText("script failed: line 3")).toBeTruthy();
    expect(screen.getByText("unknown resource kind")).toBeTruthy();
  });

  it("warns about unwarmed handler images", () => {
    render(
      <PlanDiff
        plan={{}}
        unwarmedHandlerImages={["ghcr.io/acme/tool:1", "postgres:16"]}
      />,
    );
    expect(
      screen.getByText(/handlers may pull the following images/),
    ).toBeTruthy();
    expect(screen.getByText("ghcr.io/acme/tool:1")).toBeTruthy();
    expect(screen.getByText("postgres:16")).toBeTruthy();
  });

  it("omits the image warning when the list is empty", () => {
    render(<PlanDiff plan={{}} unwarmedHandlerImages={[]} />);
    expect(screen.queryByText(/handlers may pull/)).toBeNull();
  });
});
