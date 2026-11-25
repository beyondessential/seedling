import { describe, expect, it } from "vitest";
import { formatQuantity, parseQuantity } from "./quantity";

const amount = (text: string) => parseQuantity(text);

describe("parseQuantity", () => {
  // spec: ACN#quantities
  it("multiplies the number by each suffix", () => {
    for (const [text, n] of [
      ["0", 0n],
      ["7", 7n],
      ["1k", 1_000n],
      ["1M", 1_000_000n],
      ["1E", 10n ** 18n],
      ["1Ki", 1024n],
      ["1Mi", 2n ** 20n],
      ["1Ei", 2n ** 60n],
    ] as const) {
      expect(amount(text), text).toEqual({ amount: n });
    }
  });

  // spec: ACN#quantities
  it("reads an exponent, and E alone as exa", () => {
    expect(amount("1e3")).toEqual({ amount: 1000n });
    expect(amount("1E+3")).toEqual({ amount: 1000n });
    expect(amount("15000e-3")).toEqual({ amount: 15n });
    expect(amount("2E")).toEqual({ amount: 2n * 10n ** 18n });
    expect(amount("0e99999999999999999999")).toEqual({ amount: 0n });
  });

  // spec: ACN#quantities
  it("takes a point anywhere and a sign", () => {
    expect(amount(".5Gi")).toEqual({ amount: 2n ** 29n });
    expect(amount("5.Gi")).toEqual({ amount: 5n * 2n ** 30n });
    expect(amount("1.5Gi")).toEqual({ amount: 1_610_612_736n });
    expect(amount("-512Mi")).toEqual({ amount: -512n * 2n ** 20n });
    expect(amount("+1k")).toEqual({ amount: 1000n });
  });

  // spec: ACN#quantities
  it("rejects rather than rounds or caps", () => {
    for (const text of ["1.1Ki", "1e-3", "0.5", "1e-99999999999999999999"]) {
      expect(amount(text), text).toEqual({ problem: "Must come to a whole number" });
    }
    for (const text of ["8Ei", "10E", "9223372036854775808", "1e19"]) {
      expect(amount(text), text).toEqual({ problem: "Must be within the signed 64-bit range" });
    }
    expect(amount("-8Ei")).toEqual({ amount: -(2n ** 63n) });
    expect(amount("0.5Ki")).toEqual({ amount: 512n });
  });

  // spec: ACN#quantities
  it("rejects anything else, suggesting the suffix likely meant", () => {
    for (const text of ["", " 1", "1 ", ".", "1..2", "1e", "1e3Ki", "1kk", "1u", "--1"]) {
      expect(amount(text), text).toEqual({ problem: "Must be a quantity, such as 512Mi or 1.5G" });
    }
    for (const [text, problem] of [
      ["512m", "m is milli; did you mean 512Mi?"],
      ["512MiB", "Did you mean 512Mi?"],
      ["1MB", "Did you mean 1M?"],
      ["1K", "Did you mean 1Ki?"],
      ["64B", "Did you mean 64?"],
    ]) {
      expect(amount(text), text).toEqual({ problem });
    }
  });
});

describe("formatQuantity", () => {
  // spec: ACN#quantities
  it("picks the largest exact suffix", () => {
    for (const [n, text] of [
      [0n, "0"],
      [1_000n, "1k"],
      [1_500n, "1.5k"],
      [64n * 2n ** 20n, "64Mi"],
      [15872n * 2n ** 20n, "15.5Gi"],
      [1_230n, "1.23k"],
      [1_234n, "1234"],
      [-(2n ** 63n), "-8Ei"],
    ] as const) {
      expect(formatQuantity(n), String(n)).toBe(text);
    }
  });
});
