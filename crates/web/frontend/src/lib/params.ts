import type { ParamSchema } from "./types";
import { isStrongPassword } from "./passwordStrength";

export type ParamEntries = [string, ParamSchema][];

/** The values an action's inputs start with: each param's `default_value`. */
// spec: ACN#param-schemas
export function initialParamValues(entries: ParamEntries): Record<string, string> {
  return Object.fromEntries(entries.map(([key, def]) => [key, def.default_value ?? ""]));
}

/** Default `random` generator output: 32 random bytes as 64 lowercase hex digits. */
// spec: ACN#param-kinds
export function generateRandomHex(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Why the values cannot be submitted, or null when they can.
 *
 *  A required param may not be submitted empty. A `password` value judged weak
 *  is held back because the runtime would reject it; a `weak-password` value is
 *  never held back for its strength. */
// spec: ACN#param-schemas
// spec: ACN#param-kinds
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
  }
  return null;
}

export function paramFieldType(kind: string, secret?: boolean): string {
  if (secret || kind === "password" || kind === "weak-password") return "password";
  if (kind === "email") return "email";
  return "text";
}
