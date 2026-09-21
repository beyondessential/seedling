import type { AppPriority, AppStatus, DeploymentPriority } from "./types";

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

// w[impl routes.apps.priority]
// Raised and lowered standing read differently: a raised app is one an operator
// has deliberately protected, a lowered one is the first to give way.
export function appPriorityColor(priority: AppPriority): ChipColor {
  switch (priority) {
    case "high":
      return "success";
    case "low":
      return "warning";
    case "normal":
      return "default";
  }
}

// w[impl routes.apps.priority-indicator]
// The same scale as the app chip, so a raised level reads the same in both
// places.
export function deploymentPriorityColor(priority: DeploymentPriority): ChipColor {
  switch (priority) {
    case "critical":
    case "elevated":
      return "success";
    case "low":
      return "warning";
    case "normal":
      return "default";
  }
}
