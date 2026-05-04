import { For, Show } from "solid-js";
import { Activity, ShieldAlert, Target, TrendingUp, AlertTriangle } from "lucide-solid";
import type { ActivityLog } from "../../types";

export function ActivityFeed(props: { activities: ActivityLog[] }) {
  const formatTime = (ms: number) => {
    const date = new Date(ms);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  };

  const getIcon = (level: string) => {
    switch (level) {
      case "whale": return <TrendingUp size={14} class="text-blue" />;
      case "success": return <Target size={14} class="text-green" />;
      case "warn": return <AlertTriangle size={14} class="text-orange-400" />;
      case "error": return <ShieldAlert size={14} class="text-red-500" />;
      default: return <Activity size={14} class="text-soft-text" />;
    }
  };

  const getLevelColor = (level: string) => {
    switch (level) {
      case "whale": return "text-blue bg-blue/10 border-blue/20";
      case "success": return "text-green bg-green/10 border-green/20";
      case "warn": return "text-orange-400 bg-orange-400/10 border-orange-400/20";
      case "error": return "text-red-500 bg-red-500/10 border-red-500/20";
      default: return "text-soft-text bg-soft-line/10 border-soft-line/20";
    }
  };

  return (
    <div class="bg-deep-bg border border-soft-line rounded-xl overflow-hidden h-full flex flex-col shadow-2xl">
      <div class="px-4 py-3 border-b border-soft-line flex items-center justify-between bg-black/20">
        <div class="flex items-center gap-2">
          <Activity size={16} class="text-blue" />
          <h2 class="text-sm font-bold text-white uppercase tracking-wider">Live Intelligence Feed</h2>
        </div>
        <div class="flex items-center gap-2">
            <span class="relative flex h-2 w-2">
                <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-green opacity-75"></span>
                <span class="relative inline-flex rounded-full h-2 w-2 bg-green"></span>
            </span>
            <span class="text-[0.65rem] text-soft-text font-bold uppercase">Streaming</span>
        </div>
      </div>
      
      <div class="flex-1 overflow-y-auto custom-scrollbar p-2 space-y-2">
        <Show when={props.activities.length > 0} fallback={
          <div class="h-full flex flex-col items-center justify-center text-soft-text p-8 text-center space-y-2 opacity-50">
            <Activity size={32} class="mb-2" />
            <p class="text-xs font-bold uppercase tracking-tight">Listening for signals...</p>
            <p class="text-[0.65rem]">Bot is scanning 5m markets and whale flows</p>
          </div>
        }>
          <For each={props.activities}>
            {(log) => (
              <div class={`p-2.5 rounded-lg border flex flex-col gap-1 transition-all hover:brightness-110 ${getLevelColor(log.level)}`}>
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-1.5 font-black text-[0.7rem] uppercase tracking-wide">
                    {getIcon(log.level)}
                    {log.message}
                  </div>
                  <span class="text-[0.6rem] font-mono opacity-60 font-bold">{formatTime(log.timestamp_ms)}</span>
                </div>
                
                <Show when={log.detail}>
                  <div class="text-[0.68rem] leading-relaxed font-medium pl-5 break-words opacity-90">
                    {log.detail}
                  </div>
                </Show>
              </div>
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}
