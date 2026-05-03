import type { LiveQuote, WatchedMarket } from "./types";

export function symbolFromSlug(slug: string) {
  return slug.split("-")[0]?.toUpperCase() || "UNK";
}

export function marketWindow(question: string) {
  const match = question.match(/-\s*(.+)$/);
  return match?.[1] ?? "";
}

export function outcomePrice(market: WatchedMarket, label: string) {
  const outcome = market.outcomes.find((entry) => entry.name.toLowerCase().includes(label));
  return outcome?.best_ask ?? outcome?.price;
}

export function outcomeBidAsk(market: WatchedMarket, label: string, liveQuotes: Map<string, LiveQuote>) {
  const outcome = market.outcomes.find((entry) => entry.name.toLowerCase().includes(label));
  const live = outcome?.token_id ? liveQuotes.get(outcome.token_id) : undefined;
  return {
    bid: live?.best_bid ?? outcome?.best_bid,
    ask: live?.best_ask ?? outcome?.best_ask,
  };
}

export function secondsLeft(market: WatchedMarket | undefined, lastScanAt: string | null | undefined, nowMs: number) {
  if (!market) return null;
  const scanMs = lastScanAt ? new Date(lastScanAt).getTime() : Number.NaN;
  if (Number.isNaN(scanMs)) return Math.max(0, market.seconds_to_expiry);
  const elapsed = Math.floor((nowMs - scanMs) / 1000);
  return Math.max(0, market.seconds_to_expiry - elapsed);
}
