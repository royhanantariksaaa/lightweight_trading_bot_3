import { symbolFromSlug } from "./market";
import type { Candidate, WatchedMarket } from "./types";

export function currentMarketBySymbol(markets: WatchedMarket[]) {
  const map = new Map<string, WatchedMarket>();
  for (const market of markets) {
    const symbol = symbolFromSlug(market.slug);
    const existing = map.get(symbol);
    if (!existing || market.seconds_to_expiry < existing.seconds_to_expiry) {
      map.set(symbol, market);
    }
  }
  return map;
}

export function candidatesBySlug(candidates: Candidate[]) {
  const map = new Map<string, Candidate[]>();
  for (const candidate of candidates) {
    const list = map.get(candidate.market_slug) ?? [];
    list.push(candidate);
    map.set(candidate.market_slug, list);
  }
  return map;
}

export function uniqueTokenIds(markets: WatchedMarket[]) {
  const ids = markets
    .flatMap((market) => market.outcomes)
    .map((outcome) => outcome.token_id)
    .filter((tokenId): tokenId is string => Boolean(tokenId));
  return Array.from(new Set(ids)).sort();
}

export function scanReferenceSamples(markets: WatchedMarket[], lastScanAt?: string | null) {
  const scanMs = lastScanAt ? new Date(lastScanAt).getTime() : Date.now();
  return markets
    .map((market) => ({
      symbol: symbolFromSlug(market.slug),
      value: market.current_price,
      timestamp_ms: Number.isNaN(scanMs) ? Date.now() : scanMs,
    }))
    .filter((sample): sample is { symbol: string; value: number; timestamp_ms: number } => (
      Boolean(sample.symbol) && typeof sample.value === "number" && Number.isFinite(sample.value)
    ));
}
