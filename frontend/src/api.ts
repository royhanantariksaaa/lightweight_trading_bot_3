import type { DashboardStatus } from "./types";

export async function fetchStatus(): Promise<DashboardStatus> {
  const response = await fetch("/api/status");
  if (!response.ok) throw new Error(`Status request failed: ${response.status}`);
  return response.json();
}
