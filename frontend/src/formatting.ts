export function formatTime(value?: string | null) {
  if (!value) return "not scanned";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleTimeString();
}

export function formatUsd(value: number) {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 0,
  }).format(value);
}

export function formatReferencePrice(value?: number | null) {
  if (value === null || value === undefined) return "--";
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: value >= 100 ? 2 : 4,
    maximumFractionDigits: value >= 100 ? 2 : 4,
  }).format(value);
}

export function formatDelta(value?: number | null) {
  if (value === null || value === undefined) return "--";
  const sign = value > 0 ? "+" : "";
  return `${sign}${formatReferencePrice(value)}`;
}

export function formatSpeed(value?: number | null) {
  if (value === null || value === undefined) return "--/s";
  return `${formatDelta(value)}/s`;
}
