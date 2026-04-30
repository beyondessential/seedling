import { describe, expect, it } from "vitest";
import {
  compactTargetLabel,
  logsUrlForTarget,
  parseFaultTargets,
} from "./faultTargets";

describe("parseFaultTargets", () => {
  it("returns nothing when the app is unknown", () => {
    expect(parseFaultTargets(undefined, "tamanu-api-abc12345 failed")).toEqual([]);
    expect(parseFaultTargets("tamanu", undefined)).toEqual([]);
  });

  it("extracts a target from rt.exec ensure_success output", () => {
    const targets = parseFaultTargets(
      "tamanu",
      "rt.exec command failed with exit code 3 in tamanu-api-abc12345",
    );
    expect(targets).toEqual([
      { resource: "api", instance: "abc12345", display_name: "tamanu-api-abc12345" },
    ]);
  });

  it("extracts multiple targets from rt.start.terminated output", () => {
    const targets = parseFaultTargets(
      "tamanu",
      "rt.start.terminated: resource did not terminate successfully (tamanu-api-abc12345, tamanu-tasks-deadbeef)",
    );
    expect(targets.map((t) => t.resource)).toEqual(["api", "tasks"]);
    expect(targets.map((t) => t.instance)).toEqual(["abc12345", "deadbeef"]);
  });

  it("handles resource names that contain hyphens (anonymous jobs)", () => {
    const targets = parseFaultTargets(
      "tamanu-central",
      "rt.exec command failed with exit code 1 in tamanu-central-anon-job-cafe1234",
    );
    expect(targets).toEqual([
      {
        resource: "anon-job",
        instance: "cafe1234",
        display_name: "tamanu-central-anon-job-cafe1234",
      },
    ]);
  });

  it("deduplicates repeated display_names", () => {
    const targets = parseFaultTargets(
      "tamanu",
      "tamanu-api-abc12345 failed; retry of tamanu-api-abc12345 also failed",
    );
    expect(targets).toHaveLength(1);
    expect(targets[0].display_name).toBe("tamanu-api-abc12345");
  });

  it("ignores tokens that don't end in 8 hex chars", () => {
    expect(
      parseFaultTargets("tamanu", "tamanu-api-not-actually-a-suffix").length,
    ).toBe(0);
  });

  it("requires an exact app-prefix match", () => {
    expect(parseFaultTargets("tamanu", "other-api-abc12345")).toEqual([]);
  });
});

describe("logsUrlForTarget", () => {
  it("builds a URL the Logs route accepts", () => {
    const url = logsUrlForTarget("tamanu", {
      resource: "api",
      instance: "abc12345",
      display_name: "tamanu-api-abc12345",
    });
    expect(url).toBe("/apps/tamanu/logs?resource=api&instance=abc12345");
  });
});

describe("compactTargetLabel", () => {
  it("strips the app prefix when present", () => {
    const label = compactTargetLabel("tamanu", {
      resource: "api",
      instance: "abc12345",
      display_name: "tamanu-api-abc12345",
    });
    expect(label).toBe("api-abc12345");
  });

  it("falls back to the full display_name when the prefix is absent", () => {
    const label = compactTargetLabel("other", {
      resource: "api",
      instance: "abc12345",
      display_name: "tamanu-api-abc12345",
    });
    expect(label).toBe("tamanu-api-abc12345");
  });
});
