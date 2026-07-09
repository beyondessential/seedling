import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AuthorizedKey } from "../lib/types";

const key: AuthorizedKey = {
  fingerprint: "2ed3cbdf3649b7c24d6a04b07dd052c1eca9648b28817b32b00db82df40c093e",
  label: "felix",
  // Unix timestamp in seconds: 2026-07-06T…Z. As milliseconds this would be
  // ~22 January 1970, which is exactly the bug this test guards against.
  added_at: 1783520875,
};

vi.mock("../hooks/useOi", () => ({
  useOiQuery: () => ({
    data: [key],
    loading: false,
    error: null,
    refetch: () => {},
    cachedAt: null,
  }),
}));

vi.mock("../hooks/useOiAction", () => ({
  useOiAction: () => ({
    execute: async () => {},
    loading: false,
    error: null,
    clearError: () => {},
  }),
}));

import Keys from "./Keys";

describe("Keys", () => {
  it("renders added_at as a Unix timestamp in seconds, not milliseconds", () => {
    render(<Keys />);

    const row = screen.getByText(key.label).closest("tr");
    expect(row).not.toBeNull();
    const cellText = row!.textContent ?? "";

    // The correct rendering multiplies seconds by 1000; the historical bug
    // passed the seconds value straight to `new Date()`, landing in 1970.
    const expected = new Date(key.added_at * 1000).toLocaleString();
    const buggy = new Date(key.added_at).toLocaleString();

    expect(cellText).toContain(expected);
    expect(cellText).not.toContain(buggy);
  });
});
