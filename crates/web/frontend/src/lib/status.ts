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
    case "operating":
      return "info";
    case "not_installed":
    case "uninstalling":
      return "default";
  }
}

export function statusLabel(status: AppStatus, actionName?: string): string {
  if (status === "operating" && actionName) return `operating: ${actionName}`;
  return status.replace("_", " ");
}
