import type { AppSummary } from "./types";
import type { WtClient } from "./wt";

export async function listApps(client: WtClient): Promise<AppSummary[]> {
  const result = await client.request("/apps/list", {});
  if (!result.ok) throw new Error(`[${result.error.code}] ${result.error.message}`);
  return result.value as AppSummary[];
}
