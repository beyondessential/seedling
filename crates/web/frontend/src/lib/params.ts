import type { IntRange, ParamSchema } from "./types";
import { isStrongPassword } from "./passwordStrength";

export type ParamEntries = [string, ParamSchema][];

/** The values an action's inputs start with: each param's `default_value`. */
// spec: ACN#param-schemas
export function initialParamValues(entries: ParamEntries): Record<string, string> {
  return Object.fromEntries(entries.map(([key, def]) => [key, def.default_value ?? ""]));
}

/** Default `random` generator output: 32 random bytes as 64 lowercase hex digits. */
// spec: ACN#presentation-of-kinds
export function generateRandomHex(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Why the values cannot be submitted, or null when they can.
 *
 *  A required param may not be submitted empty. A `password` value judged weak
 *  or an `integer` value outside its range is held back because the runtime
 *  would reject it; a `weak-password` value is never held back for its
 *  strength. */
// spec: ACN#param-schemas
// spec: ACN#kind-parsers
// spec: ACN#presentation-of-kinds
export function paramsBlocker(
  entries: ParamEntries,
  values: Record<string, string>,
): string | null {
  for (const [key, def] of entries) {
    const value = values[key] ?? "";
    if (value === "") {
      if (def.required) return `${key} is required`;
      continue;
    }
    if (def.kind === "password" && !isStrongPassword(value)) {
      return `${key} is too weak`;
    }
    if (def.kind === "integer" && integerProblem(value, def.range) !== null) {
      return `${key} is not a valid value`;
    }
  }
  return null;
}

export function paramFieldType(kind: string, secret?: boolean): string {
  if (secret || kind === "password" || kind === "weak-password") return "password";
  if (kind === "email") return "email";
  return "text";
}

/** Why `value` is not an `integer` value within `range`, or null when it is
 *  one or is empty. */
// spec: ACN#kind-parsers
export function integerProblem(value: string, range?: IntRange | null): string | null {
  if (value === "") return null;
  if (!/^-?(0|[1-9][0-9]*)$/.test(value) || value === "-0") return "Must be a whole number";
  const n = BigInt(value);
  const min = range?.min ?? null;
  const max = range?.max ?? null;
  if ((min !== null && n < BigInt(min)) || (max !== null && n > BigInt(max))) {
    if (min !== null && max !== null) return `Must be between ${min} and ${max}`;
    return min !== null ? `Must be at least ${min}` : `Must be at most ${max}`;
  }
  return null;
}
