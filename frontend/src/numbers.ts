export function numberValue(value: unknown) {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function normalizeTimestampMs(value: unknown) {
  const parsed = numberValue(value);
  if (parsed === undefined) return Date.now();
  return parsed < 1_000_000_000_000 ? parsed * 1000 : parsed;
}
