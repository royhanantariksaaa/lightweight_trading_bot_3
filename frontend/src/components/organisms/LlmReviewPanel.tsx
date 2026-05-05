import { Bot, Code2, FileText, RefreshCw } from "lucide-solid";
import { createSignal, For, onMount, Show } from "solid-js";
import { fetchHermesReport, fetchHermesReports } from "../../api";
import type { LlmReportDetail, LlmReportListItem } from "../../types";

export function LlmReviewPanel() {
  const [reports, setReports] = createSignal<LlmReportListItem[]>([]);
  const [selected, setSelected] = createSignal<LlmReportDetail | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [message, setMessage] = createSignal("");

  async function refreshReports() {
    setLoading(true);
    setMessage("");
    try {
      const items = await fetchHermesReports();
      setReports(items);
      if (!selected() && items[0]) void openReport(items[0].id);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }

  async function openReport(id: string) {
    setLoading(true);
    setMessage("");
    try {
      const result = await fetchHermesReport(id);
      if (!result.ok || !result.report) {
        setMessage(result.error ?? "Report could not be opened.");
        return;
      }
      setSelected(result.report);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }

  const selectedResponse = () => {
    const detail = selected();
    if (!detail) return "";
    return detail.llm_response_raw || JSON.stringify(detail.llm_response || detail.report, null, 2);
  };

  const selectedReview = () => extractReview(selected());

  onMount(() => {
    void refreshReports();
  });

  return (
    <div class="bg-deep-bg border border-soft-line rounded-xl overflow-hidden flex-1 min-h-[240px] flex flex-col shadow-2xl">
      <div class="px-4 py-3 border-b border-soft-line flex items-center justify-between bg-black/20">
        <div class="flex items-center gap-2 min-w-0">
          <Bot size={16} class="text-green" />
          <h2 class="text-sm font-bold text-white uppercase tracking-wider truncate">LLM Reviews</h2>
        </div>
        <button type="button" class="inline-flex items-center justify-center text-soft-text hover:text-white border-0 bg-transparent cursor-pointer" onClick={refreshReports} title="Refresh reports">
          <RefreshCw size={15} class={loading() ? "animate-spin" : ""} />
        </button>
      </div>

      <div class="flex-1 min-h-0 overflow-y-auto custom-scrollbar p-2 space-y-2">
        <Show when={reports().length > 0} fallback={
          <div class="h-full flex flex-col items-center justify-center text-soft-text p-6 text-center space-y-2 opacity-60">
            <FileText size={28} />
            <p class="text-xs font-bold uppercase tracking-tight">No reviews yet</p>
            <p class="text-[0.65rem]">Quant/analyst/software-engineer reviews will appear here.</p>
          </div>
        }>
          <For each={reports()}>
            {(report) => (
              <button
                type="button"
                class={`w-full text-left p-2.5 rounded-lg border bg-soft-line/10 hover:brightness-110 cursor-pointer ${selected()?.id === report.id ? "border-green" : "border-soft-line"}`}
                onClick={() => openReport(report.id)}
              >
                <div class="flex items-center justify-between gap-2">
                  <span class="min-w-0 truncate text-[0.72rem] font-black text-white">{report.question || report.market_slug || report.id}</span>
                  <Show when={report.has_code_patch}>
                    <Code2 size={13} class="text-green flex-none" />
                  </Show>
                </div>
                <div class="mt-1 flex items-center justify-between gap-2 text-[0.62rem] text-soft-text font-bold">
                  <span class="truncate">{report.market_slug || "trade/event"}</span>
                  <span class="flex-none">{formatReportTime(report.generated_at)}</span>
                </div>
              </button>
            )}
          </For>
        </Show>

        <Show when={message()}>
          <div class="p-2 rounded-md border border-red/30 bg-red/10 text-red text-[0.68rem] font-bold">{message()}</div>
        </Show>

        <Show when={selected()}>
          <div class="mt-3 border-t border-soft-line pt-3 grid gap-2">
            <div class="text-[0.7rem] font-black uppercase text-white">What the LLM wants to do</div>
            <div class="rounded-lg border border-green/30 bg-green/10 p-2 grid gap-1.5">
              <div class="flex items-center justify-between gap-2 text-[0.62rem] font-black uppercase tracking-wide">
                <span class="text-green">{selectedReview().intendedAction || "REVIEW"}</span>
                <span class="text-soft-text">{selectedReview().expectedImpact || "impact not specified"}</span>
              </div>
              <p class="text-[0.72rem] leading-relaxed text-[#d8e3ec] font-semibold">{selectedReview().summary || "No summary returned yet."}</p>
              <Show when={selectedReview().recommendations.length > 0}>
                <ul class="list-disc pl-4 text-[0.66rem] leading-relaxed text-[#b9c7d4]">
                  <For each={selectedReview().recommendations.slice(0, 4)}>
                    {(item) => <li>{item}</li>}
                  </For>
                </ul>
              </Show>
            </div>

            <div class="text-[0.7rem] font-black uppercase text-white">Raw Review JSON</div>
            <pre class="max-h-[300px] overflow-auto custom-scrollbar whitespace-pre-wrap break-words rounded-lg border border-soft-line bg-[#0d1318] p-2 text-[0.64rem] leading-relaxed text-[#b9c7d4]">{selectedResponse() || "No report data."}</pre>
          </div>
        </Show>
      </div>
    </div>
  );
}

function extractReview(detail: LlmReportDetail | null) {
  const parsed = parseResponse(detail);
  const recommendations = [
    ...stringArray(parsed?.strategy_recommendations),
    ...stringArray(parsed?.engineering_recommendations),
    ...stringArray(parsed?.risk_notes).map((risk) => `Risk: ${risk}`),
  ];
  return {
    summary: stringValue(parsed?.review_summary) || stringValue(parsed?.market_summary) || stringValue(parsed?.event_summary),
    intendedAction: stringValue(parsed?.intended_action),
    expectedImpact: stringValue(parsed?.expected_impact),
    recommendations,
  };
}

function parseResponse(detail: LlmReportDetail | null): Record<string, unknown> | null {
  if (!detail) return null;
  if (detail.llm_response && typeof detail.llm_response === "object") return detail.llm_response as Record<string, unknown>;
  if (detail.llm_response_raw) {
    try {
      const parsed = JSON.parse(detail.llm_response_raw);
      if (parsed && typeof parsed === "object") return parsed as Record<string, unknown>;
    } catch (_) {
      return null;
    }
  }
  return null;
}

function stringValue(value: unknown) {
  return typeof value === "string" ? value : "";
}

function stringArray(value: unknown) {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function formatReportTime(value?: string | null) {
  if (!value) return "--";
  return new Date(value).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
