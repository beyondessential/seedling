import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { PlanResponse } from "../lib/types";
import EditScript from "./EditScript";

vi.mock("@uiw/react-codemirror", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange?: (v: string) => void;
  }) => (
    <textarea
      aria-label="script-editor"
      value={value}
      onChange={(e) => onChange?.(e.target.value)}
    />
  ),
}));

const plan: PlanResponse = {
  diff: [{ resource_type: "deployment", resource_name: "web", change: "modified" }],
  on_change_would_fire: [],
  errors: [],
};

function renderEdit(fixtures = {}) {
  return renderWithSession(<EditScript />, {
    route: "/apps/shop/edit",
    path: "/apps/:name/edit",
    safetyMode: "write",
    fixtures: {
      "/apps/script": { script: "// v1", generation: 3 },
      "/apps/plan": plan,
      "/apps/images/discover": { per_handler: [], all_images: [] },
      "/images/list": { images: [] },
      "/images/pins/list": { pins: [] },
      ...fixtures,
    },
  });
}

async function findSeededEditor(): Promise<HTMLTextAreaElement> {
  const editor = (await screen.findByLabelText("script-editor")) as HTMLTextAreaElement;
  await waitFor(() => expect(editor.value).toBe("// v1"));
  return editor;
}

describe("EditScript", () => {
  it("loads the current script and disables review while unchanged", async () => {
    renderEdit();
    await findSeededEditor();
    expect(
      screen.getByRole("button", { name: "No changes" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("plans the change, then applies it and navigates to the app", async () => {
    const { request } = renderEdit();
    const editor = await findSeededEditor();
    fireEvent.change(editor, { target: { value: "// v2" } });
    fireEvent.click(screen.getByRole("button", { name: "Review & apply" }));
    expect(await screen.findByText(/Review changes/)).toBeTruthy();
    expect(request).toHaveBeenCalledWith("/apps/plan", {
      app: "shop",
      proposed_script: "// v2",
    });

    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/update", {
        app: "shop",
        script: "// v2",
      }),
    );
    // Navigating to /apps/shop unmounts the edit route.
    await waitFor(() =>
      expect(screen.queryByLabelText("script-editor")).toBeNull(),
    );
  });

  it("blocks apply when the plan reports errors", async () => {
    renderEdit({
      "/apps/plan": { diff: [], on_change_would_fire: [], errors: ["undefined variable x"] },
    });
    const editor = await findSeededEditor();
    fireEvent.change(editor, { target: { value: "// bad" } });
    fireEvent.click(screen.getByRole("button", { name: "Review & apply" }));
    expect(await screen.findByText(/undefined variable x/)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Apply" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("shows an error when the current script fails to load", async () => {
    renderWithSession(<EditScript />, {
      route: "/apps/shop/edit",
      path: "/apps/:name/edit",
      fixtures: {
        "/apps/script": {
          ok: false,
          error: { code: "not_found", message: "no such app" },
        },
      },
    });
    expect(await screen.findByText(/no such app/)).toBeTruthy();
  });

  // w[verify routes.apps.definition.edit]
  it("carries sidecar files over when the script is edited", async () => {
    const { request } = renderEdit({
      "/apps/script": { script: "// v1", files: ["app.seed.rhai"], generation: 3 },
      "/apps/bundle": {
        generation: 3,
        provenance: {},
        bundle: { "app.seed.rhai": btoa("// v1"), "pages/500.html": btoa("<p>") },
      },
    });
    const editor = await findSeededEditor();
    fireEvent.change(editor, { target: { value: "// v2" } });
    fireEvent.click(screen.getByRole("button", { name: "Review & apply" }));
    const expected = { "app.seed.rhai": btoa("// v2"), "pages/500.html": btoa("<p>") };
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/plan", {
        app: "shop",
        proposed_bundle: expected,
      }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/update", {
        app: "shop",
        bundle: expected,
      }),
    );
  });

  // w[verify routes.apps.definition.edit]
  it("opens a script spanning several files read-only", async () => {
    renderEdit({
      "/apps/script": {
        script: "// v1",
        files: ["lib.rhai", "app.seed.rhai"],
        generation: 3,
      },
    });
    await findSeededEditor();
    expect(screen.getByText(/spans several files/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Load file" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "No changes" }).hasAttribute("disabled"),
    ).toBe(true);
  });
});
