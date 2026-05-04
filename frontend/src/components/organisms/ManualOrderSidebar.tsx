import { ArrowDown, ArrowUp, Check, List, MousePointerClick } from "lucide-solid";
import { createSignal, For, Show } from "solid-js";
import { submitManualOrder } from "../../api";
import { outcomeBidAsk, outcomePrice } from "../../market";
import type { LiveQuote, WatchedMarket } from "../../types";

type OutcomeChoice = "Up" | "Down";

interface WalletStatus {
  cash?: number | null;
  open_orders?: any[];
}

export function ManualOrderSidebar(props: {
  live: boolean;
  wallet: WalletStatus;
  selectedMarket: WatchedMarket | undefined;
  liveQuotes: Map<string, LiveQuote>;
  onRefresh: () => void;
  compactUsd: (value?: number | null) => string;
}) {
  const [selectedOutcome, setSelectedOutcome] = createSignal<OutcomeChoice>("Up");
  const [stakeUsd, setStakeUsd] = createSignal(1);
  const [orderMessage, setOrderMessage] = createSignal("Select an active market and stake.");
  const [submitting, setSubmitting] = createSignal(false);

  async function submitTicket() {
    const market = props.selectedMarket;
    if (!market) {
      setOrderMessage("No active market selected.");
      return;
    }
    const prompt = `${props.live ? "LIVE" : "PAPER"} ${selectedOutcome()} buy on ${market.slug} for $${stakeUsd().toFixed(2)}?`;
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
      props.onRefresh();
    } catch (error) {
      setOrderMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
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

  return (
    <aside class="grid gap-3 content-start" aria-label="Manual order panel">
      <div class="p-3.5 border border-soft-line rounded-xl bg-panel shadow-[0_4px_12px_rgba(0,0,0,0.1)]">
        <div class="flex items-center justify-between pb-2.5 border-b border-soft-line">
          <strong class="flex items-center gap-1.5 text-[0.8rem] border-b-2 border-[#dfe7ef] whitespace-nowrap flex-none"><MousePointerClick size={14} /> Manual Buy</strong>
          <span class="text-[0.8rem] text-[#9aa9b7] min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-right ml-3">{props.selectedMarket?.slug ?? "No active market"}</span>
        </div>
        <div class="flex items-center gap-2.5 mt-4">
          <button
            type="button"
            class={`flex-1 h-10 inline-flex items-center justify-center gap-1.5 rounded-lg text-[0.82rem] font-heading font-semibold border border-transparent transition-all duration-200 cursor-pointer hover:-translate-y-[1px] hover:brightness-110 ${selectedOutcome() === "Up" ? "text-white bg-green" : "text-[#b8c2cb] bg-[#222b34]"}`}
            onClick={() => setSelectedOutcome("Up")}
          >
            <ArrowUp size={15} class="flex-none" />
            <span>Up {featuredPrice("up", props.selectedMarket, props.liveQuotes)}</span>
          </button>
          <button
            type="button"
            class={`flex-1 h-10 inline-flex items-center justify-center gap-1.5 rounded-lg text-[0.82rem] font-heading font-semibold border border-transparent transition-all duration-200 cursor-pointer hover:-translate-y-[1px] hover:brightness-110 ${selectedOutcome() === "Down" ? "text-white bg-red" : "text-[#b8c2cb] bg-[#222b34]"}`}
            onClick={() => setSelectedOutcome("Down")}
          >
            <ArrowDown size={15} class="flex-none" />
            <span>Down {featuredPrice("down", props.selectedMarket, props.liveQuotes)}</span>
          </button>
        </div>
        <div class="mt-3.5">
          <span class="block text-[#8999a8] text-[0.68rem] font-extrabold">{props.live ? "Live buy" : "Paper buy"}</span>
          <small class="text-[#738393] text-[0.68rem]">Balance {props.compactUsd(props.wallet.cash)}</small>
        </div>
        <div class="flex items-center gap-2 mt-3.5">
          {[1, 5, 10].map((amount) => (
            <button
              type="button"
              class={`min-w-0 flex-1 min-h-[56px] border rounded-lg transition-all duration-150 ease-in-out cursor-pointer ${stakeUsd() === amount ? 'border-blue bg-[#1a2731] text-[#e8eff6]' : 'border-soft-line bg-[#151d24] text-[#e8eff6] hover:bg-[#18232c] hover:border-[rgba(255,255,255,0.15)]'}`}
              onClick={() => setStakeUsd(amount)}
            >
              <strong class="block text-[1rem] font-bold">${amount}</strong>
              <small class="block mt-1 text-[#2dc978] text-[0.6rem]">win {props.compactUsd(payout(amount, selectedOutcome(), props.selectedMarket, props.liveQuotes))}</small>
            </button>
          ))}
        </div>
        <button
          type="button"
          class="w-full min-h-[40px] inline-flex items-center justify-center gap-[7px] mt-3.5 text-white rounded-lg bg-blue font-heading text-[0.85rem] font-bold border-none transition-all duration-150 ease-in-out hover:-translate-y-[1px] hover:brightness-110 active:translate-y-[1px] cursor-pointer disabled:cursor-not-allowed disabled:text-[#748391] disabled:bg-[#222b34] disabled:hover:translate-y-0 disabled:hover:brightness-100"
          disabled={submitting() || !props.selectedMarket}
          onClick={submitTicket}
        >
          <Check size={15} class="flex-none" />
          <span>{submitting() ? "Submitting..." : `${props.live ? "Place live" : "Prepare paper"} ${selectedOutcome()}`}</span>
        </button>
        <small class="block mt-2.5 text-[#91a0af] text-[0.68rem] leading-[1.35]">{orderMessage()}</small>
      </div>
      <div class="grid gap-2 p-3 border border-soft-line rounded-xl bg-panel">
        <strong class="flex items-center gap-1.5 text-[0.82rem]"><List size={14} /> Open orders</strong>
        <Show when={(props.wallet.open_orders?.length ?? 0) > 0} fallback={<small class="text-[#91a0af] text-[0.7rem]">No open orders.</small>}>
          <For each={props.wallet.open_orders?.slice(0, 5) ?? []}>
            {(order) => (
              <div class="flex items-center justify-between gap-3">
                <span class="text-[#91a0af] text-[0.7rem]">{order.outcome} {order.side}</span>
                <b class="text-[#f4f7fb] text-[0.75rem]">{Math.round(order.price * 100)}c</b>
              </div>
            )}
          </For>
        </Show>
      </div>
    </aside>
  );
}
