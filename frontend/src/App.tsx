import { Activity, Eye, LineChart, RefreshCw, Wifi, Zap } from "lucide-solid";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { Metric } from "./components/atoms/Metric";
import { Header } from "./components/organisms/Header";
import { ManualOrderSidebar } from "./components/organisms/ManualOrderSidebar";
import { SettingsModal } from "./components/organisms/SettingsModal";
import { SymbolPanel } from "./components/organisms/SymbolPanel";
import { ActivityFeed } from "./components/organisms/ActivityFeed";
import { symbols } from "./constants";
import { formatTime, formatUsd } from "./formatting";
import { useClock } from "./hooks/useClock";
import { useDashboardStatus } from "./hooks/useDashboardStatus";
import { useMarketQuotes } from "./hooks/useMarketQuotes";
import { useReferencePrices } from "./hooks/useReferencePrices";
import { candidatesBySlug, currentMarketBySymbol, latestWhaleByBaseSymbol, scanReferenceSamples, uniqueTokenIds } from "./selectors";

export function App() {
  const clock = useClock();
  const { status, current, refresh } = useDashboardStatus();
  const live = () => !current().dry_run && current().allow_live_buys;
  
  const marketBySymbol = createMemo(() => currentMarketBySymbol(current().watched_markets));
  const candidateBySlug = createMemo(() => candidatesBySlug(current().candidates));
  const whaleBySymbol = createMemo(() => latestWhaleByBaseSymbol(current().whale_signals));
  const tokenIds = createMemo(() => uniqueTokenIds(current().watched_markets));
  const scanSamples = createMemo(() => scanReferenceSamples(current().watched_markets, current().last_scan_at));
  
  const { liveQuotes, wsState } = useMarketQuotes(tokenIds);
  const { liveReferencePrices, rtdsState } = useReferencePrices(scanSamples);
  
  const [selectedMarketSlug, setSelectedMarketSlug] = createSignal<string | null>(null);
  const selectedMarket = createMemo(() => {
    const selected = selectedMarketSlug();
    return current().watched_markets.find((market) => market.slug === selected) ?? current().watched_markets[0];
  });
  
  const wallet = () => {
    const w = current().wallet;
    if (!w) return { positions_count: 0, error: "wallet not loaded" };
    return {
      ...w,
      portfolio_value: w.portfolio_value != null ? w.portfolio_value / 1e6 : null,
      cash: w.cash != null ? w.cash / 1e6 : null,
      allowance: w.allowance != null ? w.allowance / 1e6 : null,
    };
  };
  const compactUsd = (value?: number | null) => {
    if (value === undefined || value === null) return "--";
    if (value > 1_000_000_000) return "Unlimited";
    return formatUsd(value);
  };
  
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [clockZone, setClockZone] = createSignal<"local" | "utc">("local");
  const displayClock = createMemo(() => formatClock(clock(), clockZone()));

  createEffect(() => {
    const market = selectedMarket();
    if (market && selectedMarketSlug() !== market.slug) setSelectedMarketSlug(market.slug);
  });

  return (
    <main class="min-h-screen text-[#f4f7fb] bg-transparent">
      <Header
        live={live()}
        wallet={wallet()}
        clockZone={clockZone()}
        setClockZone={setClockZone}
        displayClock={displayClock()}
        onOpenSettings={() => setSettingsOpen(true)}
        compactUsd={compactUsd}
      />
      
      <Show when={settingsOpen()}>
        <SettingsModal current={current()} onClose={() => setSettingsOpen(false)} onRefresh={refresh} />
      </Show>

      <div class="w-[min(100%,1080px)] grid grid-cols-1 min-[1080px]:grid-cols-[1fr_300px] gap-4 min-[1080px]:gap-6 mx-auto pt-[26px] px-5 pb-3">
        <section class="min-w-0 grid gap-2.5 content-start" aria-label="Current active 5m markets">
          <div class="flex flex-col min-[720px]:flex-row min-[720px]:items-center min-[720px]:h-[38px] border border-soft-line rounded-[9px] bg-panel overflow-hidden">
            <Metric label="Mode" value={live() ? "LIVE" : "PAPER"} hot={live()} icon={Activity} />
            <Metric label="CLOB WS" value={wsState()} hot={wsState() === "live"} icon={Wifi} />
            <Metric label="RTDS" value={rtdsState()} hot={rtdsState() === "live"} icon={Zap} />
            <Metric label="Scan" value={formatTime(current().last_scan_at)} icon={Eye} />
            <Metric label="Markets" value={String(current().watched_markets.length)} icon={LineChart} />
            <button type="button" class="self-stretch min-w-[82px] min-h-[38px] inline-flex items-center justify-center gap-1.5 text-[#dbe5ee] bg-[#172028] text-[0.74rem] font-extrabold hover:brightness-110 border-0 cursor-pointer transition-all duration-200" onClick={refresh} title="Refresh scanner status">
              <RefreshCw size={15} class="flex-none" />
              <span>Refresh</span>
            </button>
          </div>

          <div class="min-h-0 grid grid-cols-1 min-[720px]:grid-cols-2 gap-2.5">
            <For each={symbols}>
              {(symbol) => {
                const market = () => marketBySymbol().get(symbol);
                const candidates = () => (market() ? candidateBySlug().get(market()!.slug) ?? [] : []);
                return (
                  <SymbolPanel
                    symbol={symbol}
                    market={market()}
                    candidates={candidates()}
                    whale={whaleBySymbol().get(symbol)}
                    live={live()}
                    lastScanAt={current().last_scan_at}
                    nowMs={clock()}
                    liveQuotes={liveQuotes()}
                    liveReferencePrices={liveReferencePrices()}
                    selected={market()?.slug === selectedMarket()?.slug}
                    onSelect={() => {
                      const activeMarket = market();
                      if (!activeMarket) return;
                      setSelectedMarketSlug(activeMarket.slug);
                    }}
                  />
                );
              }}
            </For>
          </div>

          <div class="flex items-center justify-between gap-2.5 min-h-[34px] px-3 text-[#8796a4] border border-soft-line rounded-[9px] bg-panel text-[0.72rem]">
            <span>{status.loading ? "refreshing" : "ready"}</span>
            <Show when={current().last_error} fallback={<span>{wallet().error ? `Wallet: ${wallet().error}` : "Wallet live."}</span>}>
              <span class="text-red">{current().last_error}</span>
            </Show>
          </div>
        </section>

        <aside class="flex flex-col gap-4 overflow-hidden h-[calc(100vh-100px)] sticky top-[58px]">
          <ManualOrderSidebar
            live={live()}
            wallet={wallet()}
            selectedMarket={selectedMarket()}
            liveQuotes={liveQuotes()}
            onRefresh={refresh}
            compactUsd={compactUsd}
          />
          <ActivityFeed activities={current().activities} />
        </aside>
      </div>
    </main>
  );
}

function formatClock(nowMs: number, zone: "local" | "utc") {
  const date = new Date(nowMs);
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
    timeZone: zone === "utc" ? "UTC" : undefined,
  }).format(date);
}
