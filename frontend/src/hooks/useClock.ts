import { createSignal, onCleanup, onMount } from "solid-js";

export function useClock(intervalMs = 1000) {
  const [clock, setClock] = createSignal(Date.now());

  onMount(() => {
    const timer = window.setInterval(() => setClock(Date.now()), intervalMs);
    onCleanup(() => window.clearInterval(timer));
  });

  return clock;
}
