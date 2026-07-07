import { ZxcvbnFactory } from "@zxcvbn-ts/core";
import { dictionary as commonDictionary, adjacencyGraphs } from "@zxcvbn-ts/language-common";
import { dictionary as enDictionary } from "@zxcvbn-ts/language-en";

// zxcvbn-ts v4 replaced the `zxcvbn` function + `zxcvbnOptions` singleton with a
// configured factory instance.
const zxcvbn = new ZxcvbnFactory({
  graphs: adjacencyGraphs,
  dictionary: { ...commonDictionary, ...enDictionary },
});

// Returns 0–4 matching the server-side score; >= 3 is accepted for "password" kind.
export function passwordScore(value: string): number {
  return zxcvbn.check(value).score;
}

export function isStrongPassword(value: string): boolean {
  return passwordScore(value) >= 3;
}
