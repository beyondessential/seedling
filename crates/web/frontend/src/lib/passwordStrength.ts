import { zxcvbn, zxcvbnOptions } from "@zxcvbn-ts/core";
import { dictionary as commonDictionary, adjacencyGraphs } from "@zxcvbn-ts/language-common";
import { dictionary as enDictionary } from "@zxcvbn-ts/language-en";

zxcvbnOptions.setOptions({
  graphs: adjacencyGraphs,
  dictionary: { ...commonDictionary, ...enDictionary },
});

// Returns 0–4 matching the server-side score; >= 3 is accepted for "password" kind.
export function passwordScore(value: string): number {
  return zxcvbn(value).score;
}

export function isStrongPassword(value: string): boolean {
  return passwordScore(value) >= 3;
}
