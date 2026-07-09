import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OiErrorAlert } from "./OiErrorAlert";

describe("OiErrorAlert", () => {
  it("shows the method and message", () => {
    render(
      <OiErrorAlert error={{ method: "/apps/list", message: "boom" }} />,
    );
    expect(screen.getByText(/\[OI\]\s*\/apps\/list\s*:\s*boom/)).toBeTruthy();
    expect(screen.queryByText("Stack trace")).toBeNull();
  });

  it("shows a collapsible stack trace when present", () => {
    render(
      <OiErrorAlert
        error={{
          method: "/apps/list",
          message: "boom",
          stack: "at frame one\nat frame two",
        }}
      />,
    );
    expect(screen.getByText("Stack trace")).toBeTruthy();
    expect(screen.getByText(/at frame one/)).toBeTruthy();
  });
});
