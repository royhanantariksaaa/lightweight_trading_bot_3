import { For, Show } from "solid-js";
import { formatDelta, formatReferencePrice, formatSpeed, formatUsd } from "../formatting";
import { marketWindow, outcomeBidAsk, outcomePrice, secondsLeft } from "../market";
import { freshestReferencePrice } from "../reference";
import type { Candidate, LiveQuote, LiveReferencePrice, WatchedMarket, WhaleSignal, WhaleWallInfo } from "../types";

export function SymbolPanel(props: {
  symbol: string;
  market?: WatchedMarket;
  candidates: Candidate[];
  whale?: WhaleSignal;
  live: boolean;
  lastScanAt?: string | null;
  nowMs: number;
  liveQuotes: Map<string, LiveQuote>;
  liveReferencePrices: Map<string, LiveReferencePrice>;
  selected?: boolean;
  onSelect?: () => void;
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
    <article
      class="symbol-panel"
      classList={{ active: Boolean(props.market), hot: props.candidates.length > 0, selected: props.selected }}
      role={props.market ? "button" : undefined}
      tabIndex={props.market ? 0 : undefined}
      onClick={() => props.market && props.onSelect?.()}
      onKeyDown={(event) => {
        if (!props.market || !props.onSelect) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          props.onSelect();
        }
      }}
    >
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

      <Show when={props.market} fallback={<div class="empty-market">No active 5m window found yet.</div>}>
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
              <Show when={props.whale}>
                {(whale) => <SymbolWhale signal={whale()} />}
              </Show>
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

function SymbolWhale(props: { signal: WhaleSignal }) {
  return (
    <div class="symbol-whale" classList={{ buy: props.signal.side === "BUY", sell: props.signal.side === "SELL" }}>
      <strong>{props.signal.side}</strong>
      <span>{compactUsd(props.signal.notional_usd)} {props.signal.tier}</span>
      <span>{prettySignal(props.signal.signal)}</span>
      <small>imb {signed(props.signal.imbalance_pct)}% · {recoveryNeed(props.signal)} · wall {bestWall(props.signal)}</small>
    </div>
  );
}

function recoveryNeed(signal: WhaleSignal) {
  if (signal.required_notional > 0) return compactUsd(signal.required_notional);
  return signal.side === "SELL" ? `up10 ${compactUsd(signal.need_up_10)}` : `down10 ${compactUsd(signal.need_down_10)}`;
}

function bestWall(signal: WhaleSignal) {
  const wall = largestWall(signal.bid_wall, signal.ask_wall);
  return wall ? `${compactUsd(wall.notional_usd)}@${compactPrice(wall.price)}` : "none";
}

function largestWall(left?: WhaleWallInfo | null, right?: WhaleWallInfo | null) {
  if (!left) return right;
  if (!right) return left;
  return left.notional_usd >= right.notional_usd ? left : right;
}

function prettySignal(value: string) {
  return value.replaceAll("_", " ").toLowerCase();
}

function signed(value: number) {
  const rounded = value.toFixed(1);
  return value > 0 ? `+${rounded}` : rounded;
}

function compactUsd(value: number) {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value || 0);
}

function compactPrice(value: number) {
  return new Intl.NumberFormat(undefined, {
    notation: "compact",
    maximumFractionDigits: value >= 100 ? 0 : 3,
  }).format(value || 0);
}
