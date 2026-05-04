import type { DashboardStatus, ManualOrderRequest, ManualOrderResponse, RuntimeSettingsUpdate } from "./types";

export async function fetchStatus(): Promise<DashboardStatus> {
  const response = await fetch("/api/status");
  if (!response.ok) throw new Error(`Status request failed: ${response.status}`);
  return response.json();
}

export async function submitManualOrder(order: ManualOrderRequest): Promise<ManualOrderResponse> {
  const response = await fetch("/api/manual-order", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(order),
  });
  if (!response.ok) throw new Error(`Manual order request failed: ${response.status}`);
  return response.json();
}

export async function saveSettings(settings: RuntimeSettingsUpdate): Promise<{ ok: boolean; error?: string }> {
  const response = await fetch("/api/settings", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(settings),
  });
  if (!response.ok) throw new Error(`Settings request failed: ${response.status}`);
  return response.json();
}
