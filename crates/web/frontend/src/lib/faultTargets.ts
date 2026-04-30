// Parse container display-names out of an operation_failed fault
// description so the UI can offer a one-click jump to the right
// instance's logs.
//
// The runtime now carries the failing instance's display_name in
// rt.exec / Termination ensure_success errors:
//
//   "rt.exec command failed with exit code 3 in <app>-<resource>-<8hex>"
//   "rt.start.terminated: ... (<app>-<resource>-<8hex>, <app>-<resource>-<8hex>)"
//
// where <8hex> is the 8-character lowercase-hex display_suffix derived
// from the instance UUID. We rebuild (resource, instance) pairs by
// stripping the known app prefix and the trailing hex suffix.

export interface FaultLogTarget {
  resource: string;
  instance: string;
  /** The full display_name as it appeared in the description, kept so
   *  callers can show the operator exactly what was matched. */
  display_name: string;
}

const SUFFIX = /-([0-9a-f]{8})$/;

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Extract every `<app>-<resource>-<8hex>` token from `description`,
 *  deduplicated by display_name. Returns an empty array when the app
 *  is unknown or the description carries none. */
export function parseFaultTargets(
  app: string | undefined,
  description: string | undefined,
): FaultLogTarget[] {
  if (!app || !description) return [];

  // Tokens are delimited by whitespace, commas, or parentheses in
  // the error formats we emit. We don't anchor on those characters
  // explicitly: the regex captures the longest run that starts with
  // the app prefix and ends with -<8hex>.
  const re = new RegExp(`\\b${escapeRegex(app)}-([\\w-]+)-([0-9a-f]{8})\\b`, "g");

  const out: FaultLogTarget[] = [];
  const seen = new Set<string>();
  for (const match of description.matchAll(re)) {
    // match[0] is the whole display_name. Re-derive (resource,
    // instance) from the captures to handle resource names that
    // include hyphens (e.g. anonymous jobs render as "anon-job").
    const display_name = match[0];
    if (seen.has(display_name)) continue;
    seen.add(display_name);

    // match[1] is the greedy [\w-]+ which already stripped the
    // trailing -<8hex>. match[2] is the suffix.
    const resource = match[1];
    const instance = match[2];
    out.push({ resource, instance, display_name });
  }
  return out;
}

/** Build the URL for the logs view scoped to a fault target. */
export function logsUrlForTarget(app: string, target: FaultLogTarget): string {
  const params = new URLSearchParams({
    resource: target.resource,
    instance: target.instance,
  });
  return `/apps/${app}/logs?${params.toString()}`;
}

/** Trim a display_name down to just the suffix-bearing tail for
 *  compact button labels. The full display_name is `<app>-<resource>-<8hex>`;
 *  in the UI we usually already know the app from the surrounding
 *  context, so we can show `<resource>-<8hex>` without losing info. */
export function compactTargetLabel(app: string, target: FaultLogTarget): string {
  const prefix = `${app}-`;
  return target.display_name.startsWith(prefix)
    ? target.display_name.slice(prefix.length)
    : target.display_name;
}
