import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { TemplateSummary } from "../lib/types";

// Templates embeds ScriptEditor, which wraps CodeMirror; swap the editor for
// a plain textarea so the create flow can be driven with fireEvent.
vi.mock("@uiw/react-codemirror", () => ({
  default: ({
    value,
    onChange,
  }: {
    value?: string;
    onChange?: (v: string) => void;
  }) => (
    <textarea
      aria-label="Script body"
      value={value}
      onChange={(e) => onChange?.(e.target.value)}
    />
  ),
}));

import Templates from "./Templates";

const templates: TemplateSummary[] = [
  {
    name: "postgres",
    description: "A PostgreSQL database",
    created_at: "2026-07-01T00:00:00Z",
  },
  { name: "redis-cache", description: null, created_at: "2026-07-02T00:00:00Z" },
];

describe("Templates", () => {
  it("renders the empty state", async () => {
    renderWithSession(<Templates />, { fixtures: { "/templates/list": [] } });
    expect(await screen.findByText("No templates uploaded.")).toBeTruthy();
  });

  it("renders template rows and the apps breadcrumb", async () => {
    renderWithSession(<Templates />, { fixtures: { "/templates/list": templates } });
    expect(await screen.findByText("postgres")).toBeTruthy();
    expect(screen.getByText("A PostgreSQL database")).toBeTruthy();
    expect(screen.getByText("redis-cache")).toBeTruthy();
    const crumb = screen.getByRole("link", { name: "Apps" });
    expect(crumb.getAttribute("href")).toBe("/");
  });

  it("shows an error alert when the query fails", async () => {
    renderWithSession(<Templates />, {
      fixtures: {
        "/templates/list": { ok: false, error: { code: "internal", message: "db exploded" } },
      },
    });
    expect(await screen.findByText(/db exploded/)).toBeTruthy();
  });

  it("creates a template via /templates/create in write mode", async () => {
    const { request } = renderWithSession(<Templates />, {
      fixtures: { "/templates/list": templates },
      safetyMode: "write",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Upload template" }));
    fireEvent.change(screen.getByLabelText("Template name"), {
      target: { value: "mysql" },
    });
    fireEvent.change(screen.getByLabelText("Script body"), {
      target: { value: "app.container(\"db\");" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/templates/create",
        { name: "mysql", body: "app.container(\"db\");", description: null },
      ]),
    );
  });

  it("removes a template via /templates/remove after confirmation", async () => {
    const { request } = renderWithSession(<Templates />, {
      fixtures: { "/templates/list": templates },
      safetyMode: "dangerous",
    });
    const row = (await screen.findByText("postgres")).closest("tr")!;
    fireEvent.click(within(row).getByRole("button"));
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/templates/remove",
        { name: "postgres" },
      ]),
    );
  });

  it("keeps upload and remove buttons disabled in read mode", async () => {
    renderWithSession(<Templates />, {
      fixtures: { "/templates/list": templates },
    });
    const row = (await screen.findByText("postgres")).closest("tr")!;
    expect(within(row).getByRole("button")).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Upload template" })).toHaveProperty(
      "disabled",
      true,
    );
  });
});
