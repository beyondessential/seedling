import type { AppStatus } from "./types";

type ChipColor = "success" | "warning" | "error" | "default" | "info";

export function statusColor(status: AppStatus): ChipColor {
  switch (status) {
    case "running":
      return "success";
    case "degraded":
      return "warning";
    case "faulted":
      return "error";
    case "installing":
    case "operating":
      return "info";
    case "not_installed":
    case "uninstalling":
    case "deregistering":
      return "default";
  }
}

export function statusLabel(status: AppStatus, actionName?: string): string {
  if (status === "installing") return "installing\u2026";
  if (status === "operating" && actionName) return `operating: ${actionName}`;
  return status.replace("_", " ");
}
