import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { render } from "solid-js/web";
import "./styles.css";

type Candidate = {
  market_slug: string;
  outcome: string;
  price: number;
  expected_edge: number;
  seconds_to_expiry: number;
  stake_usd: number;
  dry_run: boolean;
};

type WatchedMarket = {
  slug: string;
  question: string;
  icon?: string | null;
  image?: string | null;
  seconds_to_expiry: number;
  volume: number;
  liquidity: number;
  price_to_beat?: number | null;
  current_price?: number | null;
  outcomes: { name: string; token_id?: string | null; price: number; best_bid?: number | null; best_ask?: number | null }[];
};

type DashboardStatus = {
  last_scan_at?: string | null;
  scanned_markets: number;
  candidates: Candidate[];
  watched_markets: WatchedMarket[];
  last_error?: string | null;
  dry_run: boolean;
  allow_live_buys: boolean;
};

type LiveQuote = {
  best_bid?: number;
  best_ask?: number;
  updated_at_ms: number;
  message_timestamp_ms?: number;
};

type LiveReferencePrice = {
  value: number;
  updated_at_ms: number;
  received_at_ms: number;
  source: ReferenceSource;
  signed_speed_per_second?: number;
  avg_speed_per_second?: number;
  sample_count: number;
  samples: ReferenceSample[];
};

type ReferenceSource = "chainlink" | "scan";

type ReferenceSample = {
  value: number;
  timestamp_ms: number;
};

const symbols = ["BTC", "ETH", "SOL", "XRP"];
const emptyStatus: DashboardStatus = {
  scanned_markets: 0,
  candidates: [],
  watched_markets: [],
  dry_run: true,
  allow_live_buys: false,
};

async function fetchStatus(): Promise<DashboardStatus> {
  const response = await fetch("/api/status");
  if (!response.ok) throw new Error(`Status request failed: ${response.status}`);
  return response.json();
}

function symbolFromSlug(slug: string) {
  return slug.split("-")[0]?.toUpperCase() || "UNK";
}

function formatTime(value?: string | null) {
  if (!value) return "not scanned";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleTimeString();
}

function formatUsd(value: number) {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 0,
  }).format(value);
}

function formatReferencePrice(value?: number | null) {
  if (value === null || value === undefined) return "--";
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: value >= 100 ? 2 : 4,
    maximumFractionDigits: value >= 100 ? 2 : 4,
  }).format(value);
}

function formatDelta(value?: number | null) {
  if (value === null || value === undefined) return "--";
  const sign = value > 0 ? "+" : "";
  return `${sign}${formatReferencePrice(value)}`;
}

function formatSpeed(value?: number | null) {
  if (value === null || value === undefined) return "--/s";
  return `${formatDelta(value)}/s`;
}

function marketWindow(question: string) {
  const match = question.match(/-\s*(.+)$/);
  return match?.[1] ?? "";
}

function outcomePrice(market: WatchedMarket, label: string) {
  const outcome = market.outcomes.find((entry) => entry.name.toLowerCase().includes(label));
  return outcome?.best_ask ?? outcome?.price;
}

function outcomeBidAsk(market: WatchedMarket, label: string, liveQuotes: Map<string, LiveQuote>) {
  const outcome = market.outcomes.find((entry) => entry.name.toLowerCase().includes(label));
  const live = outcome?.token_id ? liveQuotes.get(outcome.token_id) : undefined;
  return {
    bid: live?.best_bid ?? outcome?.best_bid,
    ask: live?.best_ask ?? outcome?.best_ask,
  };
}

function App() {
  const [tick, setTick] = createSignal(0);
  const [clock, setClock] = createSignal(Date.now());
  const [liveQuotes, setLiveQuotes] = createSignal(new Map<string, LiveQuote>());
  const [liveReferencePrices, setLiveReferencePrices] = createSignal(new Map<string, LiveReferencePrice>());
  const [wsState, setWsState] = createSignal("connecting");
  const [rtdsState, setRtdsState] = createSignal("connecting");
  const [status] = createResource(tick, fetchStatus, { initialValue: emptyStatus });

  onMount(() => {
    const pollTimer = window.setInterval(() => setTick((value) => value + 1), 2500);
    const clockTimer = window.setInterval(() => setClock(Date.now()), 1000);
    onCleanup(() => {
      window.clearInterval(pollTimer);
      window.clearInterval(clockTimer);
    });
  });

  const current = () => status() ?? emptyStatus;
  const live = () => !current().dry_run && current().allow_live_buys;
  const marketBySymbol = createMemo(() => {
    const map = new Map<string, WatchedMarket>();
    for (const market of current().watched_markets) {
      const symbol = symbolFromSlug(market.slug);
      const existing = map.get(symbol);
      if (!existing || market.seconds_to_expiry < existing.seconds_to_expiry) {
        map.set(symbol, market);
      }
    }
    return map;
  });
  const candidateBySlug = createMemo(() => {
    const map = new Map<string, Candidate[]>();
    for (const candidate of current().candidates) {
      const list = map.get(candidate.market_slug) ?? [];
      list.push(candidate);
      map.set(candidate.market_slug, list);
    }
    return map;
  });
  const tokenIds = createMemo(() => {
    const ids = current()
      .watched_markets
      .flatMap((market) => market.outcomes)
      .map((outcome) => outcome.token_id)
      .filter((tokenId): tokenId is string => Boolean(tokenId));
    return Array.from(new Set(ids)).sort();
  });
  const tokenKey = createMemo(() => tokenIds().join("|"));

  createEffect(() => {
    const ids = tokenKey() ? tokenKey().split("|") : [];
    if (!ids.length) {
      setWsState("waiting");
      setLiveQuotes(new Map());
      return;
    }
    setLiveQuotes((currentQuotes) => {
      const next = new Map<string, LiveQuote>();
      for (const id of ids) {
        const quote = currentQuotes.get(id);
        if (quote) next.set(id, quote);
      }
      return next;
    });

    const socket = new WebSocket("wss://ws-subscriptions-frontend-clob.polymarket.com/ws/market");
    const subscribedIds = new Set(ids);
    let closed = false;
    let pingTimer = 0;

    socket.onopen = () => {
      setWsState("live");
      socket.send(JSON.stringify({
        assets_ids: ids,
        type: "market",
        custom_feature_enabled: true,
      }));
      pingTimer = window.setInterval(() => {
        if (socket.readyState === WebSocket.OPEN) socket.send("PING");
      }, 10000);
    };
    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || event.data === "PONG") return;
      try {
        const payload = JSON.parse(event.data);
        const messages = Array.isArray(payload) ? payload : [payload];
        for (const message of messages) applyMarketWsMessage(message, subscribedIds, setLiveQuotes);
      } catch {
        // Ignore non-JSON heartbeat payloads.
      }
    };
    socket.onerror = () => setWsState("error");
    socket.onclose = () => {
      if (!closed) setWsState("closed");
    };

    onCleanup(() => {
      closed = true;
      window.clearInterval(pingTimer);
      socket.close();
    });
  });

  createEffect(() => {
    const socket = new WebSocket("wss://ws-live-data.polymarket.com");
    let closed = false;
    let pingTimer = 0;

    socket.onopen = () => {
      setRtdsState("live");
      socket.send(JSON.stringify({
        action: "subscribe",
        subscriptions: symbols.map((symbol) => ({
          topic: "crypto_prices_chainlink",
          type: "update",
          filters: JSON.stringify({ symbol: `${symbol.toLowerCase()}/usd` }),
        })),
      }));
      pingTimer = window.setInterval(() => {
        if (socket.readyState === WebSocket.OPEN) socket.send("PING");
      }, 5000);
    };

    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || event.data === "PONG") return;
      try {
        const message = JSON.parse(event.data);
        applyReferenceWsMessage(message, setLiveReferencePrices);
      } catch {
        // Ignore non-JSON heartbeat payloads.
      }
    };

    socket.onerror = () => setRtdsState("error");
    socket.onclose = () => {
      if (!closed) setRtdsState("closed");
    };

    onCleanup(() => {
      closed = true;
      window.clearInterval(pingTimer);
      socket.close();
    });
  });

  createEffect(() => {
    const scanMs = current().last_scan_at ? new Date(current().last_scan_at!).getTime() : Date.now();
    const samples = current().watched_markets
      .map((market) => ({
        symbol: symbolFromSlug(market.slug),
        value: market.current_price,
        timestamp_ms: Number.isNaN(scanMs) ? Date.now() : scanMs,
      }))
      .filter((sample): sample is { symbol: string; value: number; timestamp_ms: number } => (
        Boolean(sample.symbol) && typeof sample.value === "number" && Number.isFinite(sample.value)
      ));

    if (!samples.length) return;

    setLiveReferencePrices((prices) => mergeReferenceUpdates(prices, samples, "scan"));
  });

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
          <button type="button" onClick={() => setTick((value) => value + 1)}>Refresh</button>
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

function Metric(props: { label: string; value: string; hot?: boolean }) {
  return (
    <div class="metric" classList={{ hot: props.hot }}>
      <span>{props.label}</span>
      <strong>{props.value}</strong>
    </div>
  );
}

function secondsLeft(market: WatchedMarket | undefined, lastScanAt: string | null | undefined, nowMs: number) {
  if (!market) return null;
  const scanMs = lastScanAt ? new Date(lastScanAt).getTime() : Number.NaN;
  if (Number.isNaN(scanMs)) return Math.max(0, market.seconds_to_expiry);
  const elapsed = Math.floor((nowMs - scanMs) / 1000);
  return Math.max(0, market.seconds_to_expiry - elapsed);
}

function SymbolPanel(props: {
  symbol: string;
  market?: WatchedMarket;
  candidates: Candidate[];
  live: boolean;
  lastScanAt?: string | null;
  nowMs: number;
  liveQuotes: Map<string, LiveQuote>;
  liveReferencePrices: Map<string, LiveReferencePrice>;
}) {
  const upBook = () => props.market ? outcomeBidAsk(props.market, "up", props.liveQuotes) : {};
  const downBook = () => props.market ? outcomeBidAsk(props.market, "down", props.liveQuotes) : {};
  const up = () => upBook().ask ?? (props.market ? outcomePrice(props.market, "up") ?? props.market.outcomes[0]?.price ?? 0 : 0);
  const down = () => downBook().ask ?? (props.market ? outcomePrice(props.market, "down") ?? props.market.outcomes[1]?.price ?? 1 - up() : 0);
  const displaySeconds = () => secondsLeft(props.market, props.lastScanAt, props.nowMs);
  const reference = () => props.liveReferencePrices.get(props.symbol);
  const currentPrice = () => freshestReferencePrice(reference(), props.market?.current_price);
  const delta = () => {
    const beat = props.market?.price_to_beat;
    const current = currentPrice();
    return beat === undefined || beat === null || current === undefined || current === null ? undefined : current - beat;
  };
  const logo = () => props.market?.icon ?? props.market?.image;
  return (
    <article class="symbol-panel" classList={{ active: Boolean(props.market), hot: props.candidates.length > 0 }}>
      <div class="symbol-head">
        <Show when={logo()} fallback={<div class="coin-fallback">{props.symbol.slice(0, 1)}</div>}>
          {(src) => <img class="coin-logo" src={src()} alt={`${props.symbol} logo`} />}
        </Show>
        <div>
          <span class="symbol-code">{props.symbol}</span>
          <strong>{props.market ? `${props.symbol} Up or Down 5m` : "waiting for market"}</strong>
          <small>{props.market ? marketWindow(props.market.question) : ""}</small>
        </div>
        <span class="time-chip">{displaySeconds() === null ? "--" : `${displaySeconds()}s`}</span>
      </div>

      <Show
        when={props.market}
        fallback={<div class="empty-market">No active 5m window found yet.</div>}
      >
        {(market) => (
          <>
            <div class="reference-grid">
              <div>
                <span>Price to beat</span>
                <strong>{formatReferencePrice(market().price_to_beat)}</strong>
              </div>
              <div class="current-reference" classList={{ positive: (delta() ?? 0) > 0, negative: (delta() ?? 0) < 0 }}>
                <span>
                  Current price
                  <em>{formatDelta(delta())}</em>
                </span>
                <strong>{formatReferencePrice(currentPrice())}</strong>
                <small class="speed-line" classList={{ positive: (reference()?.signed_speed_per_second ?? 0) > 0, negative: (reference()?.signed_speed_per_second ?? 0) < 0 }}>
                  move {formatSpeed(reference()?.signed_speed_per_second)}
                  <b>avg {formatSpeed(reference()?.avg_speed_per_second)}</b>
                </small>
              </div>
            </div>
            <div class="odds-grid">
              <div>
                <span>UP</span>
                <strong>{up().toFixed(3)}</strong>
                <small>bid {upBook().bid?.toFixed(3) ?? "--"} / ask {upBook().ask?.toFixed(3) ?? "--"}</small>
              </div>
              <div>
                <span>DOWN</span>
                <strong>{down().toFixed(3)}</strong>
                <small>bid {downBook().bid?.toFixed(3) ?? "--"} / ask {downBook().ask?.toFixed(3) ?? "--"}</small>
              </div>
            </div>
            <div class="market-meta">
              <span>Vol {formatUsd(market().volume)}</span>
              <span>Liq {formatUsd(market().liquidity)}</span>
            </div>
            <div class="signal-zone">
              <Show when={props.candidates.length} fallback={<span>No snipe signal</span>}>
                <For each={props.candidates}>
                  {(candidate) => (
                    <div class="signal-row">
                      <strong>{candidate.outcome}</strong>
                      <span>{candidate.expected_edge.toFixed(3)} edge</span>
                      <span>{candidate.dry_run || !props.live ? "paper" : "auto"}</span>
                    </div>
                  )}
                </For>
              </Show>
            </div>
          </>
        )}
      </Show>
    </article>
  );
}

function applyMarketWsMessage(
  message: any,
  subscribedIds: Set<string>,
  setLiveQuotes: (fn: (current: Map<string, LiveQuote>) => Map<string, LiveQuote>) => void,
) {
  const messageTimestamp = normalizeTimestampMs(message?.timestamp);
  if (message?.event_type === "book" && message.asset_id && subscribedIds.has(message.asset_id)) {
    updateQuote(setLiveQuotes, message.asset_id, bestBid(message.bids), bestAsk(message.asks), messageTimestamp);
    return;
  }

  if (message?.event_type === "best_bid_ask" && message.asset_id && subscribedIds.has(message.asset_id)) {
    updateQuote(setLiveQuotes, message.asset_id, numberValue(message.best_bid), numberValue(message.best_ask), messageTimestamp);
    return;
  }

  if (message?.event_type === "price_change" && Array.isArray(message.price_changes)) {
    for (const change of message.price_changes) {
      if (change.asset_id && subscribedIds.has(change.asset_id)) {
        updateQuote(setLiveQuotes, change.asset_id, numberValue(change.best_bid), numberValue(change.best_ask), messageTimestamp);
      }
    }
  }
}

function applyReferenceWsMessage(
  message: any,
  setLiveReferencePrices: (fn: (current: Map<string, LiveReferencePrice>) => Map<string, LiveReferencePrice>) => void,
) {
  if (message?.topic !== "crypto_prices_chainlink") return;

  const payload = message?.payload;
  if (!payload?.symbol) return;

  const symbol = referenceSymbol(payload.symbol);
  if (!symbol) return;
  const source = referenceSource(message.topic, payload.symbol);
  if (!source) return;

  let samples = referenceSamplesFromPayload(payload);
  let value = samples.at(-1)?.value;
  let updatedAtMs = samples.at(-1)?.timestamp_ms;

  if (!samples.length) return;

  if (value === undefined) return;

  setLiveReferencePrices((current) => mergeReferenceUpdates(current, [{
    symbol,
    value,
    timestamp_ms: updatedAtMs ?? Date.now(),
    samples,
  }], source));
}

function referenceSamplesFromPayload(payload: any): ReferenceSample[] {
  if (Array.isArray(payload.data)) {
    return payload.data
      .map((entry: any) => referenceSample(entry?.value, entry?.timestamp))
      .filter((entry: ReferenceSample | undefined): entry is ReferenceSample => Boolean(entry));
  }

  const sample = referenceSample(payload.value, payload.timestamp);
  return sample ? [sample] : [];
}

function referenceSymbol(value: unknown) {
  const raw = String(value ?? "").toLowerCase();
  if (!raw) return "";
  const slashSymbol = raw.split("/")[0]?.toUpperCase();
  if (symbols.includes(slashSymbol)) return slashSymbol;
  const usdtSymbol = raw.endsWith("usdt") ? raw.slice(0, -"usdt".length).toUpperCase() : "";
  return symbols.includes(usdtSymbol) ? usdtSymbol : "";
}

function referenceSource(topic: unknown, symbol: unknown): ReferenceSource | undefined {
  const rawTopic = String(topic ?? "");
  if (rawTopic === "crypto_prices_chainlink") return "chainlink";
  return undefined;
}

function referenceSample(valueInput: unknown, timestampInput: unknown): ReferenceSample | undefined {
  const value = numberValue(valueInput);
  if (value === undefined) return undefined;
  return {
    value,
    timestamp_ms: normalizeTimestampMs(timestampInput),
  };
}

function mergeReferenceSamples(existing: ReferenceSample[], incoming: ReferenceSample[]) {
  const byTimestamp = new Map<number, ReferenceSample>();
  for (const sample of [...existing, ...incoming]) {
    byTimestamp.set(sample.timestamp_ms, sample);
  }
  const newest = Math.max(...byTimestamp.keys(), Date.now());
  return [...byTimestamp.values()]
    .filter((sample) => newest - sample.timestamp_ms <= 90_000)
    .sort((left, right) => left.timestamp_ms - right.timestamp_ms)
    .slice(-120);
}

function mergeReferenceUpdates(
  current: Map<string, LiveReferencePrice>,
  updates: Array<{ symbol: string; value: number; timestamp_ms: number; samples?: ReferenceSample[] }>,
  source: ReferenceSource,
) {
  const next = new Map(current);
  for (const update of updates) {
    const previous = next.get(update.symbol);
    const incomingSamples = update.samples ?? [{
      value: update.value,
      timestamp_ms: update.timestamp_ms,
    }];
    const mergedSamples = mergeReferenceSamples(previous?.samples ?? [], incomingSamples);
    const speed = referenceSpeed(mergedSamples);
    const previousReceivedAt = previous?.received_at_ms ?? 0;
    const receivedAt = Date.now();
    const shouldDisplayUpdate = shouldApplyReferenceDisplayUpdate(previous, source, receivedAt);
    next.set(update.symbol, {
      value: shouldDisplayUpdate ? update.value : previous?.value ?? update.value,
      updated_at_ms: shouldDisplayUpdate ? update.timestamp_ms : previous?.updated_at_ms ?? update.timestamp_ms,
      received_at_ms: shouldDisplayUpdate ? receivedAt : previousReceivedAt,
      source: shouldDisplayUpdate ? source : previous?.source ?? source,
      signed_speed_per_second: speed.signed,
      avg_speed_per_second: speed.absolute,
      sample_count: mergedSamples.length,
      samples: mergedSamples,
    });
  }
  return next;
}

function shouldApplyReferenceDisplayUpdate(previous: LiveReferencePrice | undefined, source: ReferenceSource, receivedAt: number) {
  if (!previous) return true;
  if (source === "chainlink" && previous.source === "scan" && receivedAt - previous.received_at_ms < 3_500) return false;
  return true;
}

function freshestReferencePrice(reference: LiveReferencePrice | undefined, scanPrice?: number | null) {
  if (scanPrice === null || scanPrice === undefined) return reference?.value;
  if (!reference) return scanPrice;
  return Date.now() - reference.received_at_ms <= 3500 ? reference.value : scanPrice;
}

function referenceSpeed(samples: ReferenceSample[]) {
  if (samples.length < 2) return {};

  const newest = samples.at(-1)!.timestamp_ms;
  const windowed = samples.filter((sample) => newest - sample.timestamp_ms <= 30_000);
  const series = windowed.length >= 2 ? windowed : samples;
  const first = series[0];
  const last = series.at(-1)!;
  const seconds = Math.max(0.001, (last.timestamp_ms - first.timestamp_ms) / 1000);
  let absoluteMove = 0;

  for (let index = 1; index < series.length; index += 1) {
    absoluteMove += Math.abs(series[index].value - series[index - 1].value);
  }

  return {
    signed: (last.value - first.value) / seconds,
    absolute: absoluteMove / seconds,
  };
}

function normalizeTimestampMs(value: unknown) {
  const parsed = numberValue(value);
  if (parsed === undefined) return Date.now();
  return parsed < 1_000_000_000_000 ? parsed * 1000 : parsed;
}

function updateQuote(
  setLiveQuotes: (fn: (current: Map<string, LiveQuote>) => Map<string, LiveQuote>) => void,
  tokenId: string,
  bestBid?: number,
  bestAsk?: number,
  messageTimestampMs = Date.now(),
) {
  setLiveQuotes((current) => {
    const next = new Map(current);
    const previous = next.get(tokenId);
    if (previous?.message_timestamp_ms && messageTimestampMs < previous.message_timestamp_ms) {
      return next;
    }
    next.set(tokenId, {
      best_bid: bestBid ?? previous?.best_bid,
      best_ask: bestAsk ?? previous?.best_ask,
      updated_at_ms: Date.now(),
      message_timestamp_ms: messageTimestampMs,
    });
    return next;
  });
}

function bestBid(levels: Array<{ price?: string }> | undefined) {
  return levels
    ?.map((level) => numberValue(level.price))
    .filter((value): value is number => value !== undefined)
    .sort((left, right) => right - left)[0];
}

function bestAsk(levels: Array<{ price?: string }> | undefined) {
  return levels
    ?.map((level) => numberValue(level.price))
    .filter((value): value is number => value !== undefined)
    .sort((left, right) => left - right)[0];
}

function numberValue(value: unknown) {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
  return Number.isFinite(parsed) ? parsed : undefined;
}

render(() => <App />, document.getElementById("root")!);
