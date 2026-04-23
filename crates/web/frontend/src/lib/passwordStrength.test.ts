import { describe, expect, it } from "vitest";
import { isStrongPassword, passwordScore } from "./passwordStrength";

// w[verify auth.password]
describe("passwordStrength", () => {
  it("rates obvious weak passwords below the accept threshold", () => {
    expect(passwordScore("password")).toBeLessThan(3);
    expect(passwordScore("123456")).toBeLessThan(3);
    expect(passwordScore("")).toBeLessThan(3);
    expect(isStrongPassword("password")).toBe(false);
    expect(isStrongPassword("123456")).toBe(false);
  });

  it("accepts a strong passphrase", () => {
    // Long, non-dictionary, mixed-class — zxcvbn should score this 3+.
    expect(isStrongPassword("correct-horse-battery-staple-42!")).toBe(true);
  });

  it("returns a 0–4 score", () => {
    for (const s of ["", "abc", "p@ssw0rd", "correct horse battery staple"]) {
      const score = passwordScore(s);
      expect(score).toBeGreaterThanOrEqual(0);
      expect(score).toBeLessThanOrEqual(4);
    }
  });
});
