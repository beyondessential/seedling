import { describe, expect, it } from "vitest";
// The cases the core's quantity parser is tested against too, so the two
// cannot drift apart.
import shared from "../../../../core/src/defs/quantity/vectors.json";
import { formatQuantity, parseQuantity } from "./quantity";

type ParseCase = { text: string; amount: string } | { text: string; error: string };
const vectors = shared as {
  parse: { case: string; cases: ParseCase[] }[];
  format: { amount: string; text: string }[];
};

/** The core's error as the console words it: a sentence that starts with a
 *  plain word is capitalised, and one that starts with a suffix is not. */
const asShown = (error: string) =>
  /^(must|did) /.test(error) ? error.charAt(0).toUpperCase() + error.slice(1) : error;

describe("parseQuantity", () => {
  for (const group of vectors.parse) {
    // spec: ACN#quantities
    it(group.case, () => {
      for (const c of group.cases) {
        const expected =
          "amount" in c ? { amount: BigInt(c.amount) } : { problem: asShown(c.error) };
        expect(parseQuantity(c.text), JSON.stringify(c.text)).toEqual(expected);
      }
    });
  }
});

describe("formatQuantity", () => {
  // spec: ACN#quantities
  it("writes the shortest exact quantity, which parses back", () => {
    for (const c of vectors.format) {
      expect(formatQuantity(BigInt(c.amount)), c.amount).toBe(c.text);
      expect(parseQuantity(c.text), c.text).toEqual({ amount: BigInt(c.amount) });
    }
  });
});
