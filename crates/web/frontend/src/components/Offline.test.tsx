import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Offline } from "./Offline";

describe("Offline", () => {
  it("explains the daemon is unreachable and that retries continue", () => {
    render(<Offline />);
    expect(screen.getByText("Can't reach Seedling")).toBeTruthy();
    expect(screen.getByText(/We'll keep trying/)).toBeTruthy();
    expect(screen.getByRole("progressbar")).toBeTruthy();
  });
});
