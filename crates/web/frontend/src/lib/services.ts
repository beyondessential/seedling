import type { ServiceRef } from "./types";

/// Render an external-service target in the same shorthand the CLI uses:
/// `_site/<name>` or `<app>/<service>`.
export function formatServiceTarget(t: ServiceRef): string {
  return t.kind === "site" ? `_site/${t.name}` : `${t.app}/${t.service}`;
}

/// Best-effort client-side check that `s` is an IPv6 literal, matching the
/// OI's current dataplane restriction. The real enforcement lives server-side
/// — this just saves a round-trip for obviously-wrong values so the operator
/// gets immediate feedback in the dialog.
export function looksLikeIpv6Literal(s: string): boolean {
  if (s.length === 0) return false;
  // An IPv4 dotted quad has three dots and no colons; reject early.
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) return false;
  // An IPv6 literal always contains at least one colon. DNS names typically
  // don't, so requiring a colon rules out the common non-literal cases
  // without needing a full IPv6 grammar here.
  return s.includes(":");
}
