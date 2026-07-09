import { fireEvent, screen, waitFor } from "@testing-library/react";
import { useLocation } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { renderWithSession } from "../test/harness";
import type { TemplatePreview } from "../lib/types";
import CreateApp from "./CreateApp";

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

function LocationProbe() {
  const location = useLocation();
  return <span data-testid="pathname">{location.pathname}</span>;
}

const preview: TemplatePreview = {
  resources: [{ name: "web", type: "deployment" }],
  params: [],
  actions: [],
  script_error: null,
};

const script = 'app.deployment("web");';

function fillForm(appName = "shop") {
  fireEvent.change(screen.getByLabelText(/App name/), {
    target: { value: appName },
  });
  fireEvent.change(screen.getByLabelText("script-editor"), {
    target: { value: script },
  });
}

describe("CreateApp", () => {
  it("keeps the review button disabled in read mode", () => {
    renderWithSession(<CreateApp />);
    fillForm();
    const button = screen.getByRole("button", { name: "Review & create" });
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  it("validates the app name on blur", () => {
    renderWithSession(<CreateApp />, { safetyMode: "write" });
    const nameField = screen.getByLabelText(/App name/);
    fireEvent.change(nameField, { target: { value: "x" } });
    fireEvent.blur(nameField);
    expect(
      screen.getByText("Name must be at least 3 characters."),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("button", { name: "Review & create" })
        .hasAttribute("disabled"),
    ).toBe(true);
  });

  it("previews the script, then creates the app and navigates to it", async () => {
    const { request } = renderWithSession(
      <>
        <CreateApp />
        <LocationProbe />
      </>,
      {
        safetyMode: "write",
        fixtures: { "/templates/preview": preview },
      },
    );
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: "Review & create" }));
    expect(await screen.findByText("Review new app")).toBeTruthy();
    expect(request).toHaveBeenCalledWith("/templates/preview", { body: script });
    // The preview inventory lists the declared resource.
    expect(screen.getByText("web")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Create app" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith("/apps/create", {
        app: "shop",
        script,
      }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("pathname").textContent).toBe("/apps/shop"),
    );
  });

  it("blocks creation when the preview reports a script error", async () => {
    renderWithSession(<CreateApp />, {
      safetyMode: "write",
      fixtures: {
        "/templates/preview": { ...preview, script_error: "parse error at line 1" },
      },
    });
    fillForm();
    fireEvent.click(screen.getByRole("button", { name: "Review & create" }));
    expect(await screen.findByText("parse error at line 1")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Create app" }).hasAttribute("disabled"),
    ).toBe(true);
  });
});
