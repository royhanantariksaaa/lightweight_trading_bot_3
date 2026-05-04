import { For, Show } from "solid-js";
import { Activity, Target, TrendingUp, TrendingDown, Droplets, BarChart2, AlertCircle, ArrowUpRight, ArrowDownRight, Gauge } from "lucide-solid";
import { formatDelta, formatReferencePrice, formatSpeed, formatUsd } from "../../formatting";
import { marketWindow, outcomeBidAsk, outcomePrice, secondsLeft } from "../../market";
import { freshestReferencePrice } from "../../reference";
import type { BinanceBookInfo, Candidate, LiveQuote, LiveReferencePrice, WatchedMarket, WhaleSignal, WhaleWallInfo } from "../../types";

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
  binanceBook?: BinanceBookInfo;
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
      class="border rounded-xl min-w-0 overflow-hidden transition-all duration-150 ease-in-out focus-visible:outline-2 focus-visible:outline-[rgba(52,152,219,0.7)] focus-visible:outline-offset-2"
      classList={{
        "cursor-pointer": Boolean(props.market),
        "border-blue bg-[#1a242d] shadow-[0_0_0_1px_rgba(52,152,219,0.2),0_8px_24px_rgba(52,152,219,0.1)]": Boolean(props.market && props.selected),
        "border-soft-line bg-panel shadow-[0_4px_12px_rgba(0,0,0,0.1)] hover:-translate-y-[2px] hover:border-[rgba(52,152,219,0.5)] hover:shadow-[0_6px_16px_rgba(0,0,0,0.2)] hover:bg-[#182129]": Boolean(props.market && !props.selected),
        "!border-[rgba(243,156,18,0.5)] hover:!shadow-[0_6px_16px_rgba(0,0,0,0.2)]": Boolean(props.candidates.length > 0 && !props.selected),
        "border-soft-line bg-panel": !props.market
      }}
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
      <div class="flex items-center min-w-0 gap-2.5 px-3.5 pt-3 pb-2">
        <Show when={logo()} fallback={<div class="w-10 h-10 flex-none rounded-md bg-[#ff991f] grid place-items-center text-white text-[1.15rem] font-black">{props.symbol.slice(0, 1)}</div>}>
          {(src) => <img class="w-10 h-10 flex-none rounded-md object-cover bg-[#ff991f]" src={src()} alt={`${props.symbol} logo`} />}
        </Show>
        <div class="min-w-0 flex-1">
          <span class="hidden">{props.symbol}</span>
          <strong class="block font-heading text-[1.05rem] font-semibold leading-[1.16] text-[#f7fbff] truncate">
            {props.market ? `${props.symbol} Up or Down 5m` : "waiting for market"}
            {["XRP", "SOL", "DOGE", "PEPE"].includes(props.symbol) && (
              <span class="ml-2 text-[0.6rem] bg-red/20 text-red px-1.5 py-0.5 rounded-full uppercase tracking-tighter align-middle">High Risk</span>
            )}
          </strong>
          <small class="block mt-[3px] text-[#9aabbc] text-[0.78rem]">
            {props.market ? marketWindow(props.market.question) : ""}
          </small>
        </div>
        <span class="flex-none min-w-[44px] min-h-[24px] px-2 text-[#ff4d56] rounded-md bg-transparent text-[1.05rem] font-black border-0">
          {displaySeconds() === null ? "--" : `${displaySeconds()}s`}
        </span>
      </div>

      <Show when={props.market} fallback={<div class="min-h-[154px] grid place-items-center p-5 text-[#8796a4] text-center"><div class="grid place-items-center gap-2"><AlertCircle size={24} class="opacity-50" /> No active 5m window found yet.</div></div>}>
        {(market) => (
          <>
            <div class="grid grid-cols-2 border-t border-soft-line">
              <div class="min-w-0 py-2.5 px-3.5 border-r border-soft-line">
                <span class="flex items-center gap-1 block text-[#8999a8] text-[0.68rem] font-extrabold"><Target size={11} /> Price to beat</span>
                <strong class="block mt-[3px] font-heading text-[#c5d0db] text-[1.15rem] font-medium leading-none">{formatReferencePrice(market().price_to_beat)}</strong>
              </div>
              <div class="min-w-0 py-2.5 px-3.5">
                <span class="flex items-center gap-1 block text-[#8999a8] text-[0.68rem] font-extrabold">
                  <Activity size={11} /> Current price
                  <em class={`ml-1.5 not-italic text-[0.64rem] ${((delta() ?? 0) < 0) ? 'text-red' : 'text-[#34c87b]'}`}>{formatDelta(delta())}</em>
                </span>
                <strong class="block mt-[3px] font-heading text-amber text-[1.15rem] font-medium leading-none">{formatReferencePrice(currentPrice())}</strong>
                <small class={`flex items-center flex-wrap gap-2 mt-1.5 text-[0.65rem] font-extrabold ${((reference()?.signed_speed_per_second ?? 0) < 0) ? 'text-red' : 'text-[#34c87b]'}`}>
                  <Gauge size={10} /> move {formatSpeed(reference()?.signed_speed_per_second)}
                  <b class="text-[#6d7d8d] font-inherit">avg {formatSpeed(reference()?.avg_speed_per_second)}</b>
                </small>
              </div>
            </div>
            <div class="grid grid-cols-2 border-t border-soft-line">
              <div class="min-w-0 py-2.5 px-3.5 border-r border-soft-line">
                <span class="flex items-center gap-1 block text-[#8999a8] text-[0.68rem] font-extrabold"><TrendingUp size={11} class="text-green" /> UP</span>
                <strong class="block mt-0.5 font-heading text-green text-[1.6rem] font-semibold leading-none">{up().toFixed(3)}</strong>
                <small class="block mt-1.5 text-[#8b9baa] text-[0.66rem] font-bold">bid {upBook().bid?.toFixed(3) ?? "--"} / ask {upBook().ask?.toFixed(3) ?? "--"}</small>
              </div>
              <div class="min-w-0 py-2.5 px-3.5">
                <span class="flex items-center gap-1 block text-[#8999a8] text-[0.68rem] font-extrabold"><TrendingDown size={11} class="text-[#ff5c64]" /> DOWN</span>
                <strong class="block mt-0.5 font-heading text-[#ff5c64] text-[1.6rem] font-semibold leading-none">{down().toFixed(3)}</strong>
                <small class="block mt-1.5 text-[#8b9baa] text-[0.66rem] font-bold">bid {downBook().bid?.toFixed(3) ?? "--"} / ask {downBook().ask?.toFixed(3) ?? "--"}</small>
              </div>
            </div>
            <div class="flex items-center justify-between gap-2 py-2 px-3.5 border-t border-soft-line text-[#9aa9b7] text-[0.7rem] font-extrabold">
              <span class="flex items-center gap-1"><BarChart2 size={11} /> Vol {formatUsd(market().volume)}</span>
              <span class="flex items-center gap-1"><Droplets size={11} /> Liq {formatUsd(market().liquidity)}</span>
            </div>
            <div class="grid gap-[2px] pt-2 pb-2.5 px-3.5 border-t border-soft-line text-[#91a1af] text-[0.72rem]">
              <Show when={props.binanceBook}>
                {(book) => (
                  <div class="flex items-center justify-between gap-2 min-w-0 py-1.5 border-b border-[rgba(255,255,255,0.05)]">
                    <div class="flex items-center gap-1.5 min-w-0">
                      <BarChart2 size={13} class="text-blue flex-none" />
                      <span class="text-[0.68rem] font-black uppercase text-white/80">Binance Intel</span>
                    </div>
                    <div class="flex items-center gap-2 text-[0.66rem] font-bold text-[#9aabbc]">
                      <span class={book().imbalance_pct > 15 ? "text-green" : book().imbalance_pct < -15 ? "text-red" : ""}>
                        imb {signed(book().imbalance_pct)}%
                      </span>
                      <Show when={book().bid_wall}>
                        {(wall) => <span class="text-green/80">Wall {compactUsd(wall().notional_usd)}</span>}
                      </Show>
                      <Show when={book().ask_wall}>
                        {(wall) => <span class="text-red/80">Wall {compactUsd(wall().notional_usd)}</span>}
                      </Show>
                    </div>
                  </div>
                )}
              </Show>
              <Show when={props.whale}>
                {(whale) => <SymbolWhale signal={whale()} />}
              </Show>
              <Show when={props.candidates.length} fallback={<span>No snipe signal</span>}>
                <For each={props.candidates}>
                  {(candidate) => (
                    <div class="flex items-center justify-between gap-2 min-w-0 py-1 border-b border-[rgba(255,255,255,0.05)]">
                      <strong class="text-amber truncate">{candidate.outcome}</strong>
                      <span class="min-w-0 truncate">{candidate.expected_edge.toFixed(3)} edge</span>
                      <span class="min-w-0 truncate">{candidate.dry_run || !props.live ? "paper" : "auto"}</span>
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
    <div class="flex items-center justify-between gap-2 min-w-0 py-1 border-b border-[rgba(255,255,255,0.05)]">
      <strong class={`flex items-center gap-1 truncate ${props.signal.side === "BUY" ? "text-green" : "text-red"}`}>
        <Show when={props.signal.side === "BUY"} fallback={<ArrowDownRight size={13} />}>
          <ArrowUpRight size={13} />
        </Show>
        {props.signal.side}
      </strong>
      <span class="min-w-0 truncate">{compactUsd(props.signal.notional_usd)} {props.signal.tier}</span>
      <span class="min-w-0 truncate">{prettySignal(props.signal.signal)}</span>
      <small class="min-w-0 truncate text-[#8b9baa]">imb {signed(props.signal.imbalance_pct)}% · {recoveryNeed(props.signal)} · wall {bestWall(props.signal)}</small>
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
