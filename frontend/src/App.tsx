import { For, Show, createMemo } from "solid-js";
import { Metric } from "./components/Metric";
import { SymbolPanel } from "./components/SymbolPanel";
import { symbols } from "./constants";
import { formatTime } from "./formatting";
import { useClock } from "./hooks/useClock";
import { useDashboardStatus } from "./hooks/useDashboardStatus";
import { useMarketQuotes } from "./hooks/useMarketQuotes";
import { useReferencePrices } from "./hooks/useReferencePrices";
import { candidatesBySlug, currentMarketBySymbol, scanReferenceSamples, uniqueTokenIds } from "./selectors";

export function App() {
  const clock = useClock();
  const { status, current, refresh } = useDashboardStatus();
  const live = () => !current().dry_run && current().allow_live_buys;
  const marketBySymbol = createMemo(() => currentMarketBySymbol(current().watched_markets));
  const candidateBySlug = createMemo(() => candidatesBySlug(current().candidates));
  const tokenIds = createMemo(() => uniqueTokenIds(current().watched_markets));
  const scanSamples = createMemo(() => scanReferenceSamples(current().watched_markets, current().last_scan_at));
  const { liveQuotes, wsState } = useMarketQuotes(tokenIds);
  const { liveReferencePrices, rtdsState } = useReferencePrices(scanSamples);

  return (
    <main class="terminal">
      <header class="top-strip">
        <div class="identity">
          <span>PM</span>
          <div>
            <strong>5m Crypto Markets</strong>
            <small>CLOB V2 scanner</small>
          </div>
        </div>
        <div class="runtime">
          <Metric label="mode" value={live() ? "LIVE" : "PAPER"} hot={live()} />
          <Metric label="clob ws" value={wsState()} hot={wsState() === "live"} />
          <Metric label="rtds" value={rtdsState()} hot={rtdsState() === "live"} />
          <Metric label="scan" value={formatTime(current().last_scan_at)} />
          <Metric label="markets" value={String(current().watched_markets.length)} />
          <button type="button" onClick={refresh}>Refresh</button>
        </div>
      </header>

      <section class="symbol-grid" aria-label="Current active 5m markets">
        <For each={symbols}>
          {(symbol) => {
            const market = () => marketBySymbol().get(symbol);
            const candidates = () => (market() ? candidateBySlug().get(market()!.slug) ?? [] : []);
            return (
              <SymbolPanel
                symbol={symbol}
                market={market()}
                candidates={candidates()}
                live={live()}
                lastScanAt={current().last_scan_at}
                nowMs={clock()}
                liveQuotes={liveQuotes()}
                liveReferencePrices={liveReferencePrices()}
              />
            );
          }}
        </For>
      </section>

      <footer class="bottom-line">
        <span>{status.loading ? "refreshing" : "ready"}</span>
        <Show when={current().last_error} fallback={<span>No scanner error reported.</span>}>
          <span class="error-text">{current().last_error}</span>
        </Show>
      </footer>
    </main>
  );
}
