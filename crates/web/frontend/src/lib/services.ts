import type { ServiceRef } from "./types";

/// Render an external-service target in the same shorthand the CLI uses:
/// `_site/<name>` or `<app>/<service>`.
export function formatServiceTarget(t: ServiceRef): string {
  return t.kind === "site" ? `_site/${t.name}` : `${t.app}/${t.service}`;
}

/// Best-effort client-side check that `s` is an IPv6 literal.
export function looksLikeIpv6Literal(s: string): boolean {
  if (s.length === 0) return false;
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) return false;
  return s.includes(":");
}

/// Best-effort client-side check that `s` is an IPv4 literal.
export function looksLikeIpv4Literal(s: string): boolean {
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(s);
}

/// Best-effort client-side check that `s` is a syntactically plausible
/// site-service `remote_host`. Mirrors the rules the daemon enforces in
/// `validate_remote_host` (`crates/core/src/oi/handler/services.rs`):
/// IP literal OR DNS name (1–253 chars, dot-separated 1–63 char labels of
/// `[A-Za-z0-9-]+` not starting/ending with `-`, at least one alphabetic
/// character, no `localhost`).
export function looksLikeRemoteHost(s: string): boolean {
  if (looksLikeIpv6Literal(s)) return true;
  if (looksLikeIpv4Literal(s)) return true;
  if (s.length === 0 || s.length > 253) return false;
  if (s.toLowerCase() === "localhost") return false;
  let anyAlpha = false;
  for (const label of s.split(".")) {
    if (label.length === 0 || label.length > 63) return false;
    if (label.startsWith("-") || label.endsWith("-")) return false;
    for (const ch of label) {
      if (!/[A-Za-z0-9-]/.test(ch)) return false;
      if (/[A-Za-z]/.test(ch)) anyAlpha = true;
    }
  }
  return anyAlpha;
}

/// Format a `remote_host:remote_port` pair for display, bracketing IPv6
/// literals so the address remains parseable as a URL-style authority.
export function formatRemoteEndpoint(host: string, port: number): string {
  return looksLikeIpv6Literal(host) ? `[${host}]:${port}` : `${host}:${port}`;
}
