import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { Template } from "../lib/types";
import EditTemplate from "./EditTemplate";

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

const template: Template = {
  name: "blog",
  body: 'app.deployment("web");',
  description: "A blog",
  created_at: "2026-07-01T00:00:00Z",
};

function renderEdit(fixtures = {}) {
  return renderWithSession(<EditTemplate />, {
    route: "/templates/blog/edit",
    path: "/templates/:name/edit",
    safetyMode: "write",
    fixtures: { "/templates/show": template, ...fixtures },
  });
}

describe("EditTemplate", () => {
  it("loads the template and disables save while unchanged", async () => {
    renderEdit();
    const editor = (await screen.findByLabelText("script-editor")) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.value).toBe(template.body));
    expect((screen.getByLabelText(/Description/) as HTMLInputElement).value).toBe("A blog");
    const button = screen.getByRole("button", { name: "No changes" });
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  it("saves an edited body and navigates back to the template", async () => {
    const { request } = renderEdit();
    const editor = (await screen.findByLabelText("script-editor")) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.value).toBe(template.body));
    fireEvent.change(editor, { target: { value: "// new body" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/templates/update", {
        name: "blog",
        body: "// new body",
      }),
    );
    // Navigating to /templates/blog unmounts the edit route.
    await waitFor(() =>
      expect(screen.queryByLabelText("script-editor")).toBeNull(),
    );
  });

  it("sends only the description when the body is untouched", async () => {
    const { request } = renderEdit();
    const editor = (await screen.findByLabelText("script-editor")) as HTMLTextAreaElement;
    await waitFor(() => expect(editor.value).toBe(template.body));
    fireEvent.change(screen.getByLabelText(/Description/), {
      target: { value: "A better blog" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/templates/update", {
        name: "blog",
        description: "A better blog",
      }),
    );
  });

  it("shows an error when the template fails to load", async () => {
    renderWithSession(<EditTemplate />, {
      route: "/templates/blog/edit",
      path: "/templates/:name/edit",
      fixtures: {
        "/templates/show": {
          ok: false,
          error: { code: "not_found", message: "no such template" },
        },
      },
    });
    expect(await screen.findByText(/no such template/)).toBeTruthy();
  });
});
