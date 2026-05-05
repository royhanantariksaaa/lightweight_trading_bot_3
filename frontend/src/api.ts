import type { DashboardStatus, LlmReportDetailResponse, LlmReportListItem, ManualOrderRequest, ManualOrderResponse, RuntimeSettingsUpdate } from "./types";

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

export async function fetchHermesReports(): Promise<LlmReportListItem[]> {
  const response = await fetch("/api/llm-reports");
  if (!response.ok) throw new Error(`LLM reports request failed: ${response.status}`);
  return response.json();
}

export async function fetchHermesReport(id: string): Promise<LlmReportDetailResponse> {
  const response = await fetch(`/api/llm-reports/${encodeURIComponent(id)}`);
  if (!response.ok) throw new Error(`LLM report request failed: ${response.status}`);
  return response.json();
}
