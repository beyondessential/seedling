import type { HeldVolume, VolumeRef } from "./types";

/** A volume's display name as a volume shell mounts it: a site volume's `name`,
 *  an app volume's `<app>.<volume>`, a held volume's `held-<app>-<volume>`. */
// spec: UI#shells
export function volumeDisplayName(ref: VolumeRef, held?: Pick<HeldVolume, "app" | "volume_name">): string {
  switch (ref.kind) {
    case "site":
      return ref.name;
    case "app":
      return `${ref.app}.${ref.volume}`;
    case "held":
      return held && held.app && held.volume_name
        ? `held-${held.app}-${held.volume_name}`
        : `held-${ref.id}`;
  }
}
