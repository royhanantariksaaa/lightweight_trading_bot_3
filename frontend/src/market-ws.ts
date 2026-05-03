import { normalizeTimestampMs, numberValue } from "./numbers";
import type { LiveQuote } from "./types";

export function applyMarketWsMessage(
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
