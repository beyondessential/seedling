import { describe, expect, it } from "vitest";
import { generateRandomHex, initialParamValues, integerProblem, paramsBlocker, type ParamEntries } from "./params";

const def = (over: Record<string, unknown>) => ({
  kind: "text",
  required: true,
  description: "",
  default_value: null,
  ...over,
});

describe("initialParamValues", () => {
  // spec: ACN#param-schemas
  it("pre-fills each param with its default_value", () => {
    const entries = [
      ["a", def({ default_value: "x" })],
      ["b", def({})],
    ] as ParamEntries;
    expect(initialParamValues(entries)).toEqual({ a: "x", b: "" });
  });
});

describe("paramsBlocker", () => {
  // spec: ACN#param-schemas
  it("refuses a required param submitted empty, even one with a default", () => {
    const entries = [["a", def({ default_value: "x" })]] as ParamEntries;
    expect(paramsBlocker(entries, { a: "" })).not.toBeNull();
    expect(paramsBlocker(entries, { a: "y" })).toBeNull();
  });

  // spec: ACN#param-schemas
  it("lets an optional param be empty, a password included", () => {
    const entries = [
      ["a", def({ required: false })],
      ["p", def({ required: false, kind: "password" })],
    ] as ParamEntries;
    expect(paramsBlocker(entries, { a: "", p: "" })).toBeNull();
  });

  // spec: ACN#presentation-of-kinds
  it("never refuses a weak-password for its strength", () => {
    const entries = [["p", def({ kind: "weak-password" })]] as ParamEntries;
    expect(paramsBlocker(entries, { p: "a" })).toBeNull();
  });

  // spec: ACN#kind-parsers
  it("holds back a weak password the runtime would reject", () => {
    const entries = [["p", def({ kind: "password" })]] as ParamEntries;
    expect(paramsBlocker(entries, { p: "a" })).not.toBeNull();
    expect(paramsBlocker(entries, { p: "correct-horse-battery-staple-9X!" })).toBeNull();
  });
});

describe("generateRandomHex", () => {
  // spec: ACN#presentation-of-kinds
  it("is 32 random bytes as 64 lowercase hex digits", () => {
    const a = generateRandomHex();
    expect(a).toMatch(/^[0-9a-f]{64}$/);
    expect(generateRandomHex()).not.toBe(a);
  });
});

describe("integerProblem", () => {
  // spec: ACN#kind-parsers
  it("accepts only the decimal form within the range", () => {
    expect(integerProblem("", null)).toBeNull();
    expect(integerProblem("-12", null)).toBeNull();
    expect(integerProblem("9223372036854775807", null)).toBeNull();
    for (const value of ["01", "+1", "-0", "1.5", "1e3"]) {
      expect(integerProblem(value, null)).toBe("Must be a whole number");
    }
    expect(integerProblem("0", { min: 1, max: null })).toBe("Must be at least 1");
    expect(integerProblem("6", { min: null, max: 5 })).toBe("Must be at most 5");
    expect(integerProblem("5", { min: 1, max: 5 })).toBeNull();
  });
});
