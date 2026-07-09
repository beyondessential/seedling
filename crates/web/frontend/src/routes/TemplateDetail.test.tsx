import { fireEvent, screen, waitFor } from "@testing-library/react";
import { useLocation } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { Template, TemplatePreview } from "../lib/types";
import TemplateDetail from "./TemplateDetail";

vi.mock("@uiw/react-codemirror", () => ({
  default: ({ value }: { value: string }) => (
    <textarea aria-label="script-viewer" readOnly value={value} />
  ),
}));

function LocationProbe() {
  const location = useLocation();
  return <span data-testid="pathname">{location.pathname}</span>;
}

const template: Template = {
  name: "blog",
  body: 'app.deployment("web");',
  description: "A blog template",
  created_at: "2026-07-01T00:00:00Z",
};

const preview: TemplatePreview = {
  resources: [{ name: "web", type: "deployment" }],
  params: [],
  actions: [],
  script_error: null,
};

function renderDetail(
  options: { safetyMode?: "read" | "write" | "dangerous"; fixtures?: Record<string, unknown> } = {},
) {
  return renderWithSession(
    <>
      <TemplateDetail />
      <LocationProbe />
    </>,
    {
      route: "/templates/blog",
      path: "/templates/:name",
      fixtures: {
        "/templates/show": template,
        "/templates/preview": preview,
        ...options.fixtures,
      },
      safetyMode: options.safetyMode,
    },
  );
}

describe("TemplateDetail", () => {
  it("renders the template with preview inventory and script body", async () => {
    renderDetail();
    expect(await screen.findByText("blog")).toBeTruthy();
    expect(screen.getByText("A blog template")).toBeTruthy();
    expect(screen.getByText("Resources (1)")).toBeTruthy();
    expect(screen.getByText("web")).toBeTruthy();
    expect(
      (screen.getByLabelText("script-viewer") as HTMLTextAreaElement).value,
    ).toBe(template.body);
    // Write/dangerous actions are disabled in read mode.
    for (const name of ["Create app from template", "Edit", "Remove"]) {
      expect(
        screen.getByRole("button", { name }).hasAttribute("disabled"),
      ).toBe(true);
    }
  });

  it("shows an error when the template fails to load", async () => {
    renderDetail({
      fixtures: {
        "/templates/show": {
          ok: false,
          error: { code: "not_found", message: "no such template" },
        },
      },
    });
    expect(await screen.findByText(/no such template/)).toBeTruthy();
  });

  it("instantiates an app from the template and navigates to it", async () => {
    const { request } = renderDetail({ safetyMode: "write" });
    fireEvent.click(
      await screen.findByRole("button", { name: "Create app from template" }),
    );
    fireEvent.change(await screen.findByLabelText(/New app name/), {
      target: { value: "my-blog" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create app" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/templates/instantiate", {
        template: "blog",
        app: "my-blog",
      }),
    );
  });

  it("removes the template after confirmation in dangerous mode", async () => {
    const { request } = renderDetail({ safetyMode: "dangerous" });
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Remove template?")).toBeTruthy();
    const dialogButtons = screen.getAllByRole("button", { name: "Remove" });
    fireEvent.click(dialogButtons[dialogButtons.length - 1]);
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/templates/remove", { name: "blog" }),
    );
  });
});
