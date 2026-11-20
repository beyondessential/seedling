import { describe, expect, it } from "vitest";
import { gateStateColor, statusColor, statusLabel } from "./status";

describe("statusColor", () => {
  it("maps running to success", () => {
    expect(statusColor("running")).toBe("success");
  });

  it("maps degraded to warning", () => {
    expect(statusColor("degraded")).toBe("warning");
  });

  it("maps faulted to error", () => {
    expect(statusColor("faulted")).toBe("error");
  });

  it("maps installing and operating to info", () => {
    expect(statusColor("installing")).toBe("info");
    expect(statusColor("operating")).toBe("info");
  });

  it("maps quiescent states to default", () => {
    expect(statusColor("not_installed")).toBe("default");
    expect(statusColor("uninstalling")).toBe("default");
  });
});

describe("gateStateColor", () => {
  it("reads a satisfied gate as settled and a held one as needing attention", () => {
    expect(gateStateColor("satisfied")).toBe("success");
    expect(gateStateColor("running")).toBe("info");
    expect(gateStateColor("unsatisfied")).toBe("warning");
    expect(gateStateColor("paused")).toBe("warning");
    expect(gateStateColor("undetermined")).toBe("default");
  });
});

describe("statusLabel", () => {
  it("names a gate run by its gate", () => {
    expect(statusLabel("operating", undefined, "migrate")).toBe(
      "operating: gate migrate",
    );
  });

  it("renders installing with an ellipsis", () => {
    expect(statusLabel("installing")).toBe("installing…");
  });

  it("includes the action name for operating status", () => {
    expect(statusLabel("operating", "rotate-key")).toBe("operating: rotate-key");
  });

  it("falls back to replaced-underscore label when no action", () => {
    expect(statusLabel("not_installed")).toBe("not installed");
    expect(statusLabel("running")).toBe("running");
  });
});
