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
  it("offers a single-line text input for a version, purl, url or hostname param", () => {
    for (const kind of ["version", "version-semver", "purl", "url", "url-origin", "hostname"]) {
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

  // spec: ACN#presentation-of-kinds
  it("offers a toggle for a boolean param that an optional one can be cleared from", () => {
    function Probe({ required }: { required: boolean }) {
      const [values, setValues] = useState<Record<string, string>>({ flag: "true" });
      return (
        <>
          <ParamFields
            entries={[["flag", def({ kind: "boolean", required })]] as ParamEntries}
            values={values}
            onChange={(k, v) => setValues((s) => ({ ...s, [k]: v }))}
          />
          <output>{JSON.stringify(values.flag)}</output>
        </>
      );
    }
    const { unmount } = renderWithSession(<Probe required={false} />);
    fireEvent.click(screen.getByRole("button", { name: "false" }));
    expect(screen.getByRole("status").textContent).toBe('"false"');
    fireEvent.click(screen.getByRole("button", { name: "false" }));
    expect(screen.getByRole("status").textContent).toBe('""');
    unmount();

    renderWithSession(<Probe required={true} />);
    fireEvent.click(screen.getByRole("button", { name: "true" }));
    expect(screen.getByRole("status").textContent).toBe('"true"');
  });

  // spec: ACN#presentation-of-kinds
  it("offers a number input limited to an integer param's range", () => {
    renderWithSession(
      <Form entries={[["port", def({ kind: "integer", range: { min: 1, max: 65535 } })]] as ParamEntries} />,
    );
    const input = screen.getByLabelText(/port/, { selector: "input" }) as HTMLInputElement;
    expect(input.type).toBe("number");
    expect(input.min).toBe("1");
    expect(input.max).toBe("65535");
    fireEvent.change(input, { target: { value: "70000" } });
    expect(screen.getByText("Must be between 1 and 65535")).toBeTruthy();
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(input, { target: { value: "8080" } });
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(false);
  });

  // spec: ACN#presentation-of-kinds
  it("shows the amount a quantity resolves to, and holds back one its kind rejects", () => {
    renderWithSession(
      <Form
        entries={
          [["memory", def({ kind: "quantity", description: "Database memory", range: { min: 1, max: 2 ** 31 } })]] as ParamEntries
        }
      />,
    );
    const input = screen.getByLabelText(/memory/, { selector: "input" }) as HTMLInputElement;
    expect(input.type).toBe("text");
    fireEvent.change(input, { target: { value: "1.5Gi" } });
    expect(screen.getByText(`= ${(1_610_612_736).toLocaleString()}`)).toBeTruthy();
    expect(screen.getByText("Database memory")).toBeTruthy();
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(input, { target: { value: "32Gi" } });
    expect(screen.getByText("Must be between 1 and 2Gi")).toBeTruthy();
    expect((screen.getByText("submit") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(input, { target: { value: "512m" } });
    expect(screen.getByText("m is milli; did you mean 512Mi?")).toBeTruthy();
    expect(screen.queryByText(/^= /)).toBeNull();
  });

  // spec: ACN#presentation-of-kinds
  it("offers a choice param's options in their declared order, and none for an optional one", async () => {
    renderWithSession(
      <Form
        entries={
          [["tier", def({ kind: "choice", required: false, options: ["small", "large", "medium"] })]] as ParamEntries
        }
      />,
    );
    fireEvent.mouseDown(screen.getByRole("combobox"));
    await waitFor(() => expect(screen.getAllByRole("option").length).toBe(4));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["None", "small", "large", "medium"]);
  });
});
