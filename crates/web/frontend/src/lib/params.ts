import type { IntRange, ParamSchema } from "./types";
import { isStrongPassword, passwordScore } from "./passwordStrength";
import { formatQuantity, parseQuantity } from "./quantity";

export type ParamEntries = [string, ParamSchema][];

/** The parts of a param's schema a text input's helper line reads. */
export type TextParamSchema = Pick<ParamSchema, "kind" | "range"> & {
  description?: string | null;
};

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
 *  A required param may not be submitted empty. A `password` value judged weak,
 *  or an `integer` or `quantity` value its kind rejects, is held back because
 *  the runtime would reject it; a `weak-password` value is never held back for its
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
    if (def.kind === "quantity" && quantityProblem(value, def.range) !== null) {
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

/** Why `value` is not a `quantity` value within `range`, or null when it is
 *  one or is empty. */
// spec: ACN#quantities
export function quantityProblem(value: string, range?: IntRange | null): string | null {
  if (value === "") return null;
  const parsed = parseQuantity(value);
  if ("problem" in parsed) return parsed.problem;
  const min = range?.min ?? null;
  const max = range?.max ?? null;
  const n = parsed.amount;
  if ((min !== null && n < BigInt(min)) || (max !== null && n > BigInt(max))) {
    const bound = (b: number) => formatQuantity(BigInt(b));
    if (min !== null && max !== null) return `Must be between ${bound(min)} and ${bound(max)}`;
    return min !== null ? `Must be at least ${bound(min)}` : `Must be at most ${bound(max!)}`;
  }
  return null;
}

/** Whether a text input shows `value` as one its kind rejects: a `password`
 *  judged weak, or a `quantity` its kind refuses. */
// spec: ACN#presentation-of-kinds
export function paramValueRejected(def: TextParamSchema, value: string): boolean {
  if (def.kind === "password") return value.length > 0 && !isStrongPassword(value);
  if (def.kind === "quantity") return quantityProblem(value, def.range) !== null;
  return false;
}

/** The helper line under a text input: the reason the kind rejects the value,
 *  a `weak-password` value's strength, or else the description. */
// spec: ACN#presentation-of-kinds
export function paramHelperText(def: TextParamSchema, value: string): string | undefined {
  const description = def.description || undefined;
  if (def.kind === "quantity") return quantityProblem(value, def.range) ?? description;
  if (def.kind === "password" && value.length > 0) {
    return isStrongPassword(value) ? description : "Password is too weak";
  }
  if (def.kind === "weak-password" && value.length > 0) {
    return `Strength: ${passwordScore(value)}/4${description ? ` — ${description}` : ""}`;
  }
  return description;
}
