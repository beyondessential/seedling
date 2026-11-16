import { matchRoutes } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { routes } from "./App";

describe("routes", () => {
  // spec: UI#images
  // spec: UI#restart-history
  // spec: UI#certificates
  // spec: UI#canopy-access
  it.each(["/images", "/restarts", "/certificates", "/canopy"])("has the %s view", (path) => {
    const matches = matchRoutes(routes, path);
    expect(matches).not.toBeNull();
    expect(matches!.some((m) => m.route.path === path.slice(1))).toBe(true);
  });
});
