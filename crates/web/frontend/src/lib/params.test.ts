import { describe, expect, it } from "vitest";
import {
  generateRandomHex,
  initialParamValues,
  integerProblem,
  paramsBlocker,
  quantityProblem,
  type ParamEntries,
} from "./params";

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

describe("quantityProblem", () => {
  // spec: ACN#quantities
  it("states a range's bounds as quantities", () => {
    const range = { min: 64 * 2 ** 20, max: 15872 * 2 ** 20 };
    expect(quantityProblem("1Gi", range)).toBeNull();
    expect(quantityProblem("", range)).toBeNull();
    expect(quantityProblem("32Gi", range)).toBe("Must be between 64Mi and 15.5Gi");
    expect(quantityProblem("1Mi", { min: 1024, max: null })).toBeNull();
    expect(quantityProblem("1", { min: 1024, max: null })).toBe("Must be at least 1Ki");
    expect(quantityProblem("512m", null)).toBe("m is milli; did you mean 512Mi?");
  });

  // spec: ACN#presentation-of-kinds
  it("holds back a quantity its kind rejects", () => {
    const entries = [["size", def({ kind: "quantity", range: { min: 1, max: 1024 } })]] as ParamEntries;
    expect(paramsBlocker(entries, { size: "1Ki" })).toBeNull();
    expect(paramsBlocker(entries, { size: "2Ki" })).not.toBeNull();
    expect(paramsBlocker(entries, { size: "1m" })).not.toBeNull();
  });
});
