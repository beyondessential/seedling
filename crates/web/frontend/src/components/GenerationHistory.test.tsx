import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { GenerationEntry } from "../lib/types";
import { renderWithSession } from "../test/harness";
import { GenerationHistory } from "./GenerationHistory";

const entry = (generation: number, over: Partial<GenerationEntry> = {}): GenerationEntry => ({
  generation,
  timestamp: "2026-07-09T10:00:00Z",
  kind: "param_change",
  script_changed: false,
  ...over,
});

describe("GenerationHistory", () => {
  // spec: UI#operator-capabilities
  it("lists generations with their change and values", async () => {
    renderWithSession(<GenerationHistory appName="shop" />, {
      fixtures: {
        "/apps/generations": [
          entry(4, {
            params: [
              { name: "host", previous_value: null, new_value: "h.example" },
              { name: "port", previous_value: "80", new_value: "8080" },
              { name: "tls", previous_value: "on", new_value: null },
            ],
          }),
          entry(3, { params: [{ name: "replicas", previous_value: "1", new_value: "2" }] }),
          entry(2, { params: [{ name: "token", redacted: true }] }),
          entry(1, {
            kind: "register",
            script_changed: true,
            definition: { kind: "fetched", reference: "reg.example/shop:1", digest: "sha256:x", content_hash: "h", seedling_versions: null, metadata: {} },
          }),
        ],
      },
    });
    expect(await screen.findByText("replicas: 1 → 2")).toBeTruthy();
    // One generation lists each parameter it changed.
    expect(screen.getByText("host: (unset) → h.example")).toBeTruthy();
    expect(screen.getByText("port: 80 → 8080")).toBeTruthy();
    expect(screen.getByText("tls: on → (unset)")).toBeTruthy();
    expect(screen.getByText("values withheld")).toBeTruthy();
    expect(screen.getByText("reg.example/shop:1")).toBeTruthy();
  });

  // spec: UI#operator-capabilities
  it("shows an error rather than an empty history when the read fails", async () => {
    renderWithSession(<GenerationHistory appName="shop" />, {
      fixtures: { "/apps/generations": { ok: false, error: { code: "internal", message: "db gone" } } },
    });
    expect(await screen.findByText(/db gone/)).toBeTruthy();
    expect(screen.queryByText("No generations.")).toBeNull();
  });

  // spec: UI#operator-capabilities
  it("pages back from the oldest generation shown", async () => {
    const page = Array.from({ length: 50 }, (_, i) => entry(100 - i));
    const { request } = renderWithSession(<GenerationHistory appName="shop" />, {
      fixtures: {
        "/apps/generations": (params: unknown) =>
          (params as { before?: number }).before === undefined ? page : [entry(50)],
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Show older" }));
    await waitFor(() =>
      expect(request.mock.calls.filter(([m]) => m === "/apps/generations").map(([, p]) => p)).toContainEqual({
        app: "shop",
        limit: 50,
        before: 51,
      }),
    );
    await waitFor(() => expect(screen.queryByRole("button", { name: "Show older" })).toBeNull());
  });
});
