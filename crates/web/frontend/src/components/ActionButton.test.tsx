import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import {
  IconActionButton,
  OutlinedActionButton,
  SolidActionButton,
} from "./ActionButton";

function button(name: string): HTMLButtonElement {
  return screen.getByRole("button", { name }) as HTMLButtonElement;
}

describe("ActionButton safety guards", () => {
  it("read-tier buttons are enabled in read mode and fire onClick", () => {
    const onClick = vi.fn();
    renderWithSession(
      <SolidActionButton safety="read" onClick={onClick}>
        Refresh
      </SolidActionButton>,
    );
    expect(button("Refresh").disabled).toBe(false);
    fireEvent.click(button("Refresh"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  // w[verify sessions.safety-mode]
  it("write-tier buttons are disabled in read mode", () => {
    const onClick = vi.fn();
    renderWithSession(
      <SolidActionButton safety="write" onClick={onClick}>
        Deploy
      </SolidActionButton>,
    );
    expect(button("Deploy").disabled).toBe(true);
    fireEvent.click(button("Deploy"));
    expect(onClick).not.toHaveBeenCalled();
  });

  // w[verify sessions.safety-mode]
  it("write-tier buttons are enabled in write mode", () => {
    const onClick = vi.fn();
    renderWithSession(
      <OutlinedActionButton safety="write" onClick={onClick}>
        Deploy
      </OutlinedActionButton>,
      { safetyMode: "write" },
    );
    expect(button("Deploy").disabled).toBe(false);
    fireEvent.click(button("Deploy"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  // w[verify sessions.safety-mode]
  it("dangerous-tier buttons stay disabled in write mode", () => {
    renderWithSession(
      <SolidActionButton safety="dangerous">Delete</SolidActionButton>,
      { safetyMode: "write" },
    );
    expect(button("Delete").disabled).toBe(true);
  });

  // w[verify sessions.safety-mode]
  it("dangerous-tier buttons are enabled in dangerous mode", () => {
    const onClick = vi.fn();
    renderWithSession(
      <SolidActionButton safety="dangerous" onClick={onClick}>
        Delete
      </SolidActionButton>,
      { safetyMode: "dangerous" },
    );
    expect(button("Delete").disabled).toBe(false);
    fireEvent.click(button("Delete"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("external disabled prop wins even when the mode allows the tier", () => {
    renderWithSession(
      <SolidActionButton safety="write" disabled>
        Deploy
      </SolidActionButton>,
      { safetyMode: "dangerous" },
    );
    expect(button("Deploy").disabled).toBe(true);
  });

  // w[verify sessions.safety-mode]
  it("icon buttons follow the same guard", () => {
    const onClick = vi.fn();
    const { unmount } = renderWithSession(
      <IconActionButton safety="write" aria-label="remove row" onClick={onClick}>
        x
      </IconActionButton>,
    );
    expect(button("remove row").disabled).toBe(true);
    unmount();

    renderWithSession(
      <IconActionButton safety="write" aria-label="remove row" onClick={onClick}>
        x
      </IconActionButton>,
      { safetyMode: "write" },
    );
    expect(button("remove row").disabled).toBe(false);
    fireEvent.click(button("remove row"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
