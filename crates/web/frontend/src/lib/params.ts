import type { DurationRange, IntRange, ParamSchema } from "./types";
import { isStrongPassword, passwordScore } from "./passwordStrength";
import { formatQuantity, parseQuantity } from "./quantity";

export type ParamEntries = [string, ParamSchema][];

/** The parts of a param's schema a text input's helper line reads. */
export type TextParamSchema = Pick<ParamSchema, "kind" | "range" | "precision"> & {
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
    if (def.kind === "integer" && integerProblem(value, intRange(def)) !== null) {
      return `${key} is not a valid value`;
    }
    if (def.kind === "quantity" && quantityProblem(value, intRange(def)) !== null) {
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

/** The parts of a schema that say which values its kind accepts. */
interface KindBounds {
  kind: string;
  range?: IntRange | DurationRange | null;
  precision?: string | null;
}

/** An `integer` or `quantity` kind's range, or null for any other kind or none. */
export function intRange(def: KindBounds): IntRange | null {
  return def.kind === "integer" || def.kind === "quantity"
    ? ((def.range as IntRange | null | undefined) ?? null)
    : null;
}

const PRECISION_UNITS: Record<string, string> = {
  d: "days",
  h: "hours",
  m: "minutes",
  s: "seconds",
  ms: "milliseconds",
  us: "microseconds",
  ns: "nanoseconds",
};

/** The bounds a `duration` kind declares, as helper text, or null for any
 *  other kind or one with no bounds. */
// spec: ACN#presentation-of-kinds
export function durationBounds(def: KindBounds): string | null {
  if (def.kind !== "duration") return null;
  const range = (def.range as DurationRange | null | undefined) ?? null;
  const min = range?.min ?? null;
  const max = range?.max ?? null;
  const parts: string[] = [];
  if (min !== null && max !== null) parts.push(`${min} to ${max}`);
  else if (min !== null) parts.push(`At least ${min}`);
  else if (max !== null) parts.push(`At most ${max}`);
  if (def.precision) {
    const unit = PRECISION_UNITS[def.precision] ?? def.precision;
    parts.push(parts.length > 0 ? `whole ${unit}` : `Whole ${unit}`);
  }
  return parts.length > 0 ? parts.join(", ") : null;
}

/** A param's description, after its duration bounds where it has any. */
export function describeParam(
  def: KindBounds & { description?: string | null },
): string | undefined {
  const bounds = durationBounds(def);
  if (bounds !== null && def.description) return `${bounds}. ${def.description}`;
  return bounds ?? (def.description || undefined);
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
  if (def.kind === "quantity") return quantityProblem(value, intRange(def)) !== null;
  return false;
}

/** The helper line under a text input: the reason the kind rejects the value,
 *  a `weak-password` value's strength, or else the description. */
// spec: ACN#presentation-of-kinds
export function paramHelperText(def: TextParamSchema, value: string): string | undefined {
  const description = describeParam(def);
  if (def.kind === "quantity") return quantityProblem(value, intRange(def)) ?? description;
  if (def.kind === "password" && value.length > 0) {
    return isStrongPassword(value) ? description : "Password is too weak";
  }
  if (def.kind === "weak-password" && value.length > 0) {
    return `Strength: ${passwordScore(value)}/4${description ? ` — ${description}` : ""}`;
  }
  return description;
}
