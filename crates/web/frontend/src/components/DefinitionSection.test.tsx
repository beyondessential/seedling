import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { DefinitionProvenance, FaultRecord, PlanResponse } from "../lib/types";
import { renderWithSession } from "../test/harness";
import { DefinitionSection } from "./DefinitionSection";
import { referenceFor, splitReference } from "./UpdateDefinitionDialog";

const fetched: DefinitionProvenance = {
  kind: "fetched",
  reference: "ghcr.io/org/central-def:v2.11.3",
  digest: "sha256:4f1c9a02",
  content_hash: "sha256:aaaa",
  seedling_versions: ">=0.12",
};

const pushed: DefinitionProvenance = {
  kind: "pushed",
  pushed_by: { kind: "ctl", id: "fp1", display: "Alex" },
  reported_origin: {
    url: "https://github.com/org/repo/tree/main/deploy",
    revision: "3e1f2a9",
  },
  content_hash: "sha256:0d9e77b4",
  seedling_versions: null,
};

const moved: FaultRecord = {
  id: "f1",
  app: "central",
  kind: "definition_source_moved",
  timestamp: "2026-09-24T00:00:00Z",
  description:
    "ghcr.io/org/central-def:v2.11.3 now selects sha256:b73e, but the app runs sha256:4f1c9a02",
};

function mount(
  definition: DefinitionProvenance,
  faults: FaultRecord[] = [],
  fixtures: Record<string, unknown> = {},
) {
  return renderWithSession(
    <DefinitionSection
      appName="central"
      definition={definition}
      faults={faults}
      onUpdated={() => {}}
    />,
    { safetyMode: "write", fixtures },
  );
}

const plan: PlanResponse = {
  diff: [{ resource_type: "Deployment", resource_name: "api", change: "modified", fields: ["image"] }],
  on_change_would_fire: ["version"],
  errors: [],
  rejections: [],
};

describe("DefinitionSection", () => {
  // w[verify routes.apps.definition]
  it("shows a fetched definition's reference and digest", () => {
    mount(fetched);
    expect(screen.getByText("ghcr.io/org/central-def:v2.11.3")).toBeTruthy();
    expect(screen.getByText("sha256:4f1c9a02")).toBeTruthy();
    expect(screen.getByText(">=0.12")).toBeTruthy();
  });

  // w[verify routes.apps.definition]
  it("shows a pushed definition's hash, pusher, and reported origin", () => {
    mount(pushed);
    expect(screen.getByText("Alex (ctl)")).toBeTruthy();
    expect(screen.getByText("sha256:0d9e77b4")).toBeTruthy();
    expect(screen.getByText("https://github.com/org/repo/tree/main/deploy")).toBeTruthy();
    expect(screen.getByText("(reported by client)")).toBeTruthy();
  });

  // w[verify routes.apps.definition.fetch]
  it("flags a moved tag and offers it as the one to update to", async () => {
    const { request } = mount(fetched, [moved], {
      "/registries/tags": { tags: ["v2.11.2", "v2.11.3", "v2.12.0"] },
    });
    expect(screen.getByText(/now selects sha256:b73e/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    const input = (await screen.findByLabelText("Tag or reference")) as HTMLInputElement;
    expect(input.value).toBe("v2.11.3");
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/registries/tags", {
        repository: "ghcr.io/org/central-def",
      }),
    );
  });

  // w[verify routes.apps.definition.fetch]
  it("plans, then applies the definition and one param in one update", async () => {
    const { request } = mount(fetched, [], {
      "/registries/tags": { tags: ["v2.11.3", "v2.12.0"] },
      "/apps/plan": plan,
      "/apps/update": { schedule: "accepted", generation: 43 },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update from registry" }));
    const input = await screen.findByLabelText("Tag or reference");
    fireEvent.change(input, { target: { value: "v2.12.0" } });
    fireEvent.change(screen.getByLabelText("Param"), { target: { value: "version" } });
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "2.12.0" } });
    const apply = screen.getByRole("button", { name: "Apply" });
    expect(apply.hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/plan", {
        app: "central",
        proposed_reference: "ghcr.io/org/central-def:v2.12.0",
        proposed_params: [{ name: "version", value: "2.12.0" }],
      }),
    );
    await waitFor(() => expect(apply.hasAttribute("disabled")).toBe(false));
    fireEvent.click(apply);
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/update", {
        app: "central",
        reference: "ghcr.io/org/central-def:v2.12.0",
        param: { name: "version", value: "2.12.0" },
      }),
    );
    expect(request.mock.calls.filter((c) => c[0] === "/apps/update")).toHaveLength(1);
  });

  // w[verify routes.apps.definition.fetch]
  it("shows a validator rejection and keeps Apply disabled", async () => {
    mount(fetched, [], {
      "/registries/tags": { tags: [] },
      "/apps/plan": {
        ...plan,
        rejections: [{ name: "hostname", reason: "needs a fully qualified name" }],
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update from registry" }));
    fireEvent.change(await screen.findByLabelText("Tag or reference"), {
      target: { value: "v2.12.0" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/needs a fully qualified name/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(true);
  });

  // w[verify routes.apps.definition.fetch]
  it("takes a complete reference for a pushed definition", async () => {
    const { request } = mount(pushed, [], { "/apps/plan": plan });
    fireEvent.click(screen.getByRole("button", { name: "Update from registry" }));
    fireEvent.change(await screen.findByLabelText("Reference"), {
      target: { value: "ghcr.io/org/central-def:v2.12.0" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/plan", {
        app: "central",
        proposed_reference: "ghcr.io/org/central-def:v2.12.0",
      }),
    );
    expect(request.mock.calls.some((c) => c[0] === "/registries/tags")).toBe(false);
  });
});

describe("reference helpers", () => {
  it("splits a reference into repository and tag", () => {
    expect(splitReference("ghcr.io/o/d:1.2")).toEqual({ repository: "ghcr.io/o/d", tag: "1.2" });
    expect(splitReference("localhost:5000/o/d@sha256:ab")).toEqual({
      repository: "localhost:5000/o/d",
      tag: null,
    });
  });

  it("builds a reference from a tag or passes a complete one through", () => {
    expect(referenceFor("v2", "ghcr.io/o/d")).toBe("ghcr.io/o/d:v2");
    expect(referenceFor("quay.io/x/y:1", "ghcr.io/o/d")).toBe("quay.io/x/y:1");
    expect(referenceFor("v2", null)).toBeNull();
    expect(referenceFor("  ", "ghcr.io/o/d")).toBeNull();
  });
});
