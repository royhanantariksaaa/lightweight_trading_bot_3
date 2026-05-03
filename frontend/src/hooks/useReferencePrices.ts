import { Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import { symbols } from "../constants";
import { applyReferenceWsMessage, mergeReferenceUpdates } from "../reference";
import type { LiveReferencePrice } from "../types";

const RTDS_URL = "wss://ws-live-data.polymarket.com";

export function useReferencePrices(scanSamples: Accessor<Array<{ symbol: string; value: number; timestamp_ms: number }>>) {
  const [liveReferencePrices, setLiveReferencePrices] = createSignal(new Map<string, LiveReferencePrice>());
  const [rtdsState, setRtdsState] = createSignal("connecting");

  createEffect(() => {
    const socket = new WebSocket(RTDS_URL);
    let closed = false;
    let pingTimer = 0;

    socket.onopen = () => {
      setRtdsState("live");
      socket.send(JSON.stringify({
        action: "subscribe",
        subscriptions: symbols.map((symbol) => ({
          topic: "crypto_prices_chainlink",
          type: "update",
          filters: JSON.stringify({ symbol: `${symbol.toLowerCase()}/usd` }),
        })),
      }));
      pingTimer = window.setInterval(() => {
        if (socket.readyState === WebSocket.OPEN) socket.send("PING");
      }, 5000);
    };

    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || event.data === "PONG") return;
      try {
        applyReferenceWsMessage(JSON.parse(event.data), setLiveReferencePrices);
      } catch {
        // Ignore non-JSON heartbeat payloads.
      }
    };

    socket.onerror = () => setRtdsState("error");
    socket.onclose = () => {
      if (!closed) setRtdsState("closed");
    };

    onCleanup(() => {
      closed = true;
      window.clearInterval(pingTimer);
      socket.close();
    });
  });

  createEffect(() => {
    const samples = scanSamples();
    if (!samples.length) return;
    setLiveReferencePrices((prices) => mergeReferenceUpdates(prices, samples, "scan"));
  });

  return { liveReferencePrices, rtdsState };
}
