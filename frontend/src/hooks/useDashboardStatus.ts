import { createResource, createSignal, onCleanup, onMount } from "solid-js";
import { fetchStatus } from "../api";
import { emptyStatus } from "../constants";

export function useDashboardStatus(pollMs = 2500) {
  const [tick, setTick] = createSignal(0);
  const [status] = createResource(tick, fetchStatus, { initialValue: emptyStatus });

  onMount(() => {
    const timer = window.setInterval(() => refresh(), pollMs);
    onCleanup(() => window.clearInterval(timer));
  });

  function refresh() {
    setTick((value) => value + 1);
  }

  return {
    status,
    current: () => status() ?? emptyStatus,
    refresh,
  };
}
