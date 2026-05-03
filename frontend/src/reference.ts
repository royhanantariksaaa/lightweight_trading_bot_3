import { symbols } from "./constants";
import { normalizeTimestampMs, numberValue } from "./numbers";
import type { LiveReferencePrice, ReferenceSample, ReferenceSource } from "./types";

export function applyReferenceWsMessage(
  message: any,
  setLiveReferencePrices: (fn: (current: Map<string, LiveReferencePrice>) => Map<string, LiveReferencePrice>) => void,
) {
  if (message?.topic !== "crypto_prices_chainlink") return;

  const payload = message?.payload;
  if (!payload?.symbol) return;

  const symbol = referenceSymbol(payload.symbol);
  if (!symbol) return;

  const source = referenceSource(message.topic);
  if (!source) return;

  const samples = referenceSamplesFromPayload(payload);
  const value = samples.at(-1)?.value;
  const updatedAtMs = samples.at(-1)?.timestamp_ms;

  if (!samples.length || value === undefined) return;

  setLiveReferencePrices((current) =>
    mergeReferenceUpdates(current, [{
      symbol,
      value,
      timestamp_ms: updatedAtMs ?? Date.now(),
      samples,
    }], source),
  );
}

export function referenceSamplesFromPayload(payload: any): ReferenceSample[] {
  if (Array.isArray(payload.data)) {
    return payload.data
      .map((entry: any) => referenceSample(entry?.value, entry?.timestamp))
      .filter((entry: ReferenceSample | undefined): entry is ReferenceSample => Boolean(entry));
  }

  const sample = referenceSample(payload.value, payload.timestamp);
  return sample ? [sample] : [];
}

export function mergeReferenceUpdates(
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

export function freshestReferencePrice(reference: LiveReferencePrice | undefined, scanPrice?: number | null) {
  if (scanPrice === null || scanPrice === undefined) return reference?.value;
  if (!reference) return scanPrice;
  return Date.now() - reference.received_at_ms <= 3500 ? reference.value : scanPrice;
}

function referenceSymbol(value: unknown) {
  const raw = String(value ?? "").toLowerCase();
  if (!raw) return "";
  const slashSymbol = raw.split("/")[0]?.toUpperCase();
  if (symbols.includes(slashSymbol)) return slashSymbol;
  const usdtSymbol = raw.endsWith("usdt") ? raw.slice(0, -"usdt".length).toUpperCase() : "";
  return symbols.includes(usdtSymbol) ? usdtSymbol : "";
}

function referenceSource(topic: unknown): ReferenceSource | undefined {
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

function shouldApplyReferenceDisplayUpdate(previous: LiveReferencePrice | undefined, source: ReferenceSource, receivedAt: number) {
  if (!previous) return true;
  if (source === "chainlink" && previous.source === "scan" && receivedAt - previous.received_at_ms < 3_500) return false;
  return true;
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
