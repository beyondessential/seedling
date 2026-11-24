import { fireEvent, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { initialParamValues, paramsBlocker, type ParamEntries } from "../lib/params";
import { renderWithSession } from "../test/harness";
import { ParamFields } from "./ParamFields";

const def = (over: Record<string, unknown>) => ({
  kind: "text",
  required: true,
  description: "",
  default_value: null,
  ...over,
});

function Form({ entries }: { entries: ParamEntries }) {
  const [values, setValues] = useState(() => initialParamValues(entries));
  return (
    <>
      <ParamFields
        entries={entries}
        values={values}
        onChange={(k, v) => setValues((s) => ({ ...s, [k]: v }))}
      />
      <button disabled={paramsBlocker(entries, values) !== null}>submit</button>
    </>
  );
}

describe("ParamFields", () => {
  // spec: ACN#param-schemas
  it("pre-fills an input with its default_value and holds back an emptied required one", () => {
    renderWithSession(<Form entries={[["name", def({ default_value: "main" })]] as ParamEntries} />);
    const input = screen.getByLabelText(/name/) as HTMLInputElement;
    expect(input.value).toBe("main");
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(input, { target: { value: "" } });
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(true);
  });

  // spec: ACN#presentation-of-kinds
  it("generates 64 lowercase hex digits for a random param", () => {
    renderWithSession(<Form entries={[["token", def({ kind: "random" })]] as ParamEntries} />);
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("Generate token"));
    const input = screen.getByLabelText(/token/, { selector: "input" }) as HTMLInputElement;
    expect(input.value).toMatch(/^[0-9a-f]{64}$/);
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(false);
  });

  // spec: ACN#presentation-of-kinds
  it("shows the strength of a weak-password but still lets it be submitted", () => {
    renderWithSession(<Form entries={[["pw", def({ kind: "weak-password" })]] as ParamEntries} />);
    fireEvent.change(screen.getByLabelText(/pw/, { selector: "input" }), { target: { value: "a" } });
    expect(screen.getByText(/Strength: \d\/4/)).toBeTruthy();
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(false);
  });

  // spec: ACN#presentation-of-kinds
  it("offers a single-line text input for a version param", () => {
    for (const kind of ["version", "version-semver"]) {
      const { unmount } = renderWithSession(<Form entries={[["tag", def({ kind })]] as ParamEntries} />);
      const input = screen.getByLabelText(/tag/, { selector: "input" }) as HTMLInputElement;
      expect(input.type).toBe("text");
      unmount();
    }
  });

  // spec: ACN#volume-params
  it("offers site volumes for a volume param", async () => {
    renderWithSession(<Form entries={[["data", def({ kind: "volume" })]] as ParamEntries} />, {
      fixtures: { "/volumes/site/list": [{ name: "bigdisk", kind: "bind" }] },
    });
    fireEvent.mouseDown(screen.getByRole("combobox"));
    await waitFor(() => expect(screen.getByRole("option", { name: /bigdisk/ })).toBeTruthy());
  });
});
