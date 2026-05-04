import { ArrowDown, ArrowUp, Bot, Check, Clock3, RefreshCw, Save, Settings, X } from "lucide-solid";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { saveSettings, submitManualOrder } from "./api";
import { Metric } from "./components/Metric";
import { SymbolPanel } from "./components/SymbolPanel";
import { symbols } from "./constants";
import { formatTime, formatUsd } from "./formatting";
import { useClock } from "./hooks/useClock";
import { useDashboardStatus } from "./hooks/useDashboardStatus";
import { useMarketQuotes } from "./hooks/useMarketQuotes";
import { useReferencePrices } from "./hooks/useReferencePrices";
import { outcomeBidAsk, outcomePrice } from "./market";
import { candidatesBySlug, currentMarketBySymbol, latestWhaleByBaseSymbol, scanReferenceSamples, uniqueTokenIds } from "./selectors";
import type { LiveQuote, WatchedMarket } from "./types";

type OutcomeChoice = "Up" | "Down";

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
  const wallet = () => current().wallet ?? { positions_count: 0, error: "wallet not loaded" };
  const compactUsd = (value?: number | null) => value === undefined || value === null ? "--" : formatUsd(value);
  const [selectedOutcome, setSelectedOutcome] = createSignal<OutcomeChoice>("Up");
  const [stakeUsd, setStakeUsd] = createSignal(1);
  const [orderMessage, setOrderMessage] = createSignal("Select an active market and stake.");
  const [submitting, setSubmitting] = createSignal(false);
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [draftLive, setDraftLive] = createSignal(false);
  const [draftPrivateKey, setDraftPrivateKey] = createSignal("");
  const [draftFunder, setDraftFunder] = createSignal("");
  const [draftSignatureType, setDraftSignatureType] = createSignal("");
  const [draftMaxOrder, setDraftMaxOrder] = createSignal(5);
  const [settingsMessage, setSettingsMessage] = createSignal("");
  const [savingSettings, setSavingSettings] = createSignal(false);
  const [clockZone, setClockZone] = createSignal<"local" | "utc">("local");
  const displayClock = createMemo(() => formatClock(clock(), clockZone()));

  createEffect(() => {
    const market = selectedMarket();
    if (market && selectedMarketSlug() !== market.slug) setSelectedMarketSlug(market.slug);
  });

  function openSettings() {
    setDraftLive(!current().dry_run && current().allow_live_buys);
    setDraftFunder(current().funder_address ?? "");
    setDraftSignatureType(current().signature_type === null || current().signature_type === undefined ? "" : String(current().signature_type));
    setDraftMaxOrder(current().live_max_order_usd || 5);
    setDraftPrivateKey("");
    setSettingsMessage("");
    setSettingsOpen(true);
  }

  async function submitSettings() {
    if (draftLive() && !current().wallet_configured && !draftPrivateKey().trim()) {
      setSettingsMessage("Live mode needs a private key.");
      return;
    }
    const signatureType = draftSignatureType().trim() === "" ? null : Number(draftSignatureType());
    setSavingSettings(true);
    setSettingsMessage("Saving settings...");
    try {
      const result = await saveSettings({
        dry_run: !draftLive(),
        allow_live_buys: draftLive(),
        live_max_order_usd: draftMaxOrder(),
        funder_address: draftFunder(),
        signature_type: signatureType,
        private_key: draftPrivateKey().trim() ? draftPrivateKey() : null,
      });
      if (!result.ok) {
        setSettingsMessage(result.error ?? "Settings were rejected.");
        return;
      }
      setSettingsMessage("Settings saved.");
      setDraftPrivateKey("");
      refresh();
    } catch (error) {
      setSettingsMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setSavingSettings(false);
    }
  }

  async function submitTicket() {
    const market = selectedMarket();
    if (!market) {
      setOrderMessage("No active market selected.");
      return;
    }
    const prompt = `${live() ? "LIVE" : "PAPER"} ${selectedOutcome()} buy on ${market.slug} for $${stakeUsd().toFixed(2)}?`;
    if (!window.confirm(prompt)) return;

    setSubmitting(true);
    setOrderMessage("Submitting order...");
    try {
      const result = await submitManualOrder({
        market_slug: market.slug,
        outcome: selectedOutcome(),
        amount_usd: stakeUsd(),
      });
      setOrderMessage(result.message);
      refresh();
    } catch (error) {
      setOrderMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <main class="polymarket-shell">
      <header class="top-strip">
        <div class="identity">
          <span class="mark"><Bot size={16} strokeWidth={2.4} /></span>
          <div>
            <strong>5m Snipe Bot</strong>
            <small>{live() ? "live trading enabled" : "paper trading"}</small>
          </div>
        </div>
        <div class="wallet-strip">
          <Metric label="Portfolio" value={compactUsd(wallet().portfolio_value)} hot={!wallet().error} />
          <Metric label="Cash" value={compactUsd(wallet().cash)} hot={!wallet().error} />
          <Metric label="Positions" value={String(wallet().positions_count)} />
          <Metric label="Open Orders" value={String(wallet().open_orders?.length ?? 0)} />
          <Metric label="Allowance" value={compactUsd(wallet().allowance)} hot={!wallet().error} />
          <button
            type="button"
            class="clock-button"
            onClick={() => setClockZone((zone) => zone === "local" ? "utc" : "local")}
            title="Switch local/UTC time"
          >
            <span>{clockZone() === "local" ? "Local" : "UTC"}</span>
            <strong><Clock3 size={13} />{displayClock()}</strong>
          </button>
          <button type="button" class="settings-button" onClick={openSettings} title="Open settings">
            <Settings size={15} />
            <span>Settings</span>
          </button>
        </div>
      </header>
      <Show when={settingsOpen()}>
        <div class="settings-backdrop" onClick={() => setSettingsOpen(false)}>
          <section class="settings-popover" onClick={(event) => event.stopPropagation()}>
            <div class="settings-head">
              <strong>Settings</strong>
              <button type="button" onClick={() => setSettingsOpen(false)} title="Close settings">
                <X size={15} />
                <span>Close</span>
              </button>
            </div>
            <label class="toggle-row">
              <span>
                <strong>Trading mode</strong>
                <small>{draftLive() ? "Live orders enabled" : "Paper orders only"}</small>
              </span>
              <input type="checkbox" checked={draftLive()} onInput={(event) => setDraftLive(event.currentTarget.checked)} />
            </label>
            <label>
              <span>Polymarket private key</span>
              <input
                type="password"
                autocomplete="off"
                placeholder={current().wallet_configured ? "Configured. Enter new key to replace." : "Required for wallet and live orders"}
                value={draftPrivateKey()}
                onInput={(event) => setDraftPrivateKey(event.currentTarget.value)}
              />
            </label>
            <label>
              <span>Funder address</span>
              <input value={draftFunder()} onInput={(event) => setDraftFunder(event.currentTarget.value)} placeholder="Optional proxy/funder address" />
            </label>
            <label>
              <span>Signature type</span>
              <select value={draftSignatureType()} onInput={(event) => setDraftSignatureType(event.currentTarget.value)}>
                <option value="">EOA / default</option>
                <option value="1">Proxy</option>
                <option value="2">Gnosis Safe</option>
                <option value="3">Poly 1271</option>
              </select>
            </label>
            <label>
              <span>Max live order</span>
              <input type="number" min="1" step="1" value={draftMaxOrder()} onInput={(event) => setDraftMaxOrder(Number(event.currentTarget.value || 0))} />
            </label>
            <button type="button" class="save-settings" disabled={savingSettings()} onClick={submitSettings}>
              <Save size={15} />
              <span>{savingSettings() ? "Saving..." : "Save settings"}</span>
            </button>
            <small class="settings-note">{settingsMessage() || "Private key stays in this local backend process and is never returned in status."}</small>
          </section>
        </div>
      </Show>

      <div class="market-layout">
        <section class="market-workspace" aria-label="Current active 5m markets">
          <div class="runtime">
            <Metric label="Mode" value={live() ? "LIVE" : "PAPER"} hot={live()} />
            <Metric label="CLOB WS" value={wsState()} hot={wsState() === "live"} />
            <Metric label="RTDS" value={rtdsState()} hot={rtdsState() === "live"} />
            <Metric label="Scan" value={formatTime(current().last_scan_at)} />
            <Metric label="Markets" value={String(current().watched_markets.length)} />
            <button type="button" onClick={refresh} title="Refresh scanner status">
              <RefreshCw size={15} />
              <span>Refresh</span>
            </button>
          </div>

          <div class="symbol-grid">
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
                      setOrderMessage(`${symbol} selected.`);
                    }}
                  />
                );
              }}
            </For>
          </div>

          <div class="bottom-line">
            <span>{status.loading ? "refreshing" : "ready"}</span>
            <Show when={current().last_error} fallback={<span>{wallet().error ? `Wallet: ${wallet().error}` : "Wallet live."}</span>}>
              <span class="error-text">{current().last_error}</span>
            </Show>
          </div>
        </section>

        <aside class="trade-rail" aria-label="Manual order panel">
          <div class="ticket">
            <div class="ticket-tabs">
              <strong>Manual Buy</strong>
              <span>{selectedMarket()?.slug ?? "No active market"}</span>
            </div>
            <div class="ticket-actions">
              <button
                type="button"
                classList={{ "up-buy": selectedOutcome() === "Up" }}
                onClick={() => setSelectedOutcome("Up")}
              >
                <ArrowUp size={15} />
                <span>Up {featuredPrice("up", selectedMarket(), liveQuotes())}</span>
              </button>
              <button
                type="button"
                classList={{ "down-buy": selectedOutcome() === "Down" }}
                onClick={() => setSelectedOutcome("Down")}
              >
                <ArrowDown size={15} />
                <span>Down {featuredPrice("down", selectedMarket(), liveQuotes())}</span>
              </button>
            </div>
            <div class="ticket-balance">
              <span>{live() ? "Live buy" : "Paper buy"}</span>
              <small>Balance {compactUsd(wallet().cash)}</small>
            </div>
            <div class="stake-grid">
              <button type="button" classList={{ selected: stakeUsd() === 1 }} onClick={() => setStakeUsd(1)}><strong>$1</strong><small>win {compactUsd(payout(1, selectedOutcome(), selectedMarket(), liveQuotes()))}</small></button>
              <button type="button" classList={{ selected: stakeUsd() === 5 }} onClick={() => setStakeUsd(5)}><strong>$5</strong><small>win {compactUsd(payout(5, selectedOutcome(), selectedMarket(), liveQuotes()))}</small></button>
              <button type="button" classList={{ selected: stakeUsd() === 10 }} onClick={() => setStakeUsd(10)}><strong>$10</strong><small>win {compactUsd(payout(10, selectedOutcome(), selectedMarket(), liveQuotes()))}</small></button>
            </div>
            <button
              type="button"
              class="submit-ticket"
              disabled={submitting() || !selectedMarket()}
              onClick={submitTicket}
            >
              <Check size={15} />
              <span>{submitting() ? "Submitting..." : `${live() ? "Place live" : "Prepare paper"} ${selectedOutcome()}`}</span>
            </button>
            <small class="ticket-message">{orderMessage()}</small>
          </div>
          <div class="open-orders">
            <strong>Open orders</strong>
            <Show when={(wallet().open_orders?.length ?? 0) > 0} fallback={<small>No open orders.</small>}>
              <For each={wallet().open_orders?.slice(0, 5) ?? []}>
                {(order) => (
                  <div class="open-order-row">
                    <span>{order.outcome} {order.side}</span>
                    <b>{Math.round(order.price * 100)}c</b>
                  </div>
                )}
              </For>
            </Show>
          </div>
        </aside>
      </div>
    </main>
  );
}

function featuredPrice(side: "up" | "down", market: WatchedMarket | undefined, liveQuotes: Map<string, LiveQuote>) {
  if (!market) return "--";
  const book = outcomeBidAsk(market, side, liveQuotes);
  const price = book.ask ?? outcomePrice(market, side);
  return price === undefined || price === null ? "--" : `${Math.round(price * 100)}c`;
}

function payout(stake: number, outcome: OutcomeChoice, market: WatchedMarket | undefined, liveQuotes: Map<string, LiveQuote>) {
  if (!market) return undefined;
  const side = outcome.toLowerCase() as "up" | "down";
  const book = outcomeBidAsk(market, side, liveQuotes);
  const price = book.ask ?? outcomePrice(market, side);
  return price && price > 0 ? stake / price : undefined;
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
