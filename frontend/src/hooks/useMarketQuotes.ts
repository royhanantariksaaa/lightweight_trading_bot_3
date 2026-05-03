import { Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import { applyMarketWsMessage } from "../market-ws";
import type { LiveQuote } from "../types";

const MARKET_WS_URL = "wss://ws-subscriptions-frontend-clob.polymarket.com/ws/market";

export function useMarketQuotes(tokenIds: Accessor<string[]>) {
  const [liveQuotes, setLiveQuotes] = createSignal(new Map<string, LiveQuote>());
  const [wsState, setWsState] = createSignal("connecting");

  createEffect(() => {
    const ids = tokenIds();
    if (!ids.length) {
      setWsState("waiting");
      setLiveQuotes(new Map());
      return;
    }

    retainSubscribedQuotes(ids, setLiveQuotes);

    const socket = new WebSocket(MARKET_WS_URL);
    const subscribedIds = new Set(ids);
    let closed = false;
    let pingTimer = 0;

    socket.onopen = () => {
      setWsState("live");
      socket.send(JSON.stringify({
        assets_ids: ids,
        type: "market",
        custom_feature_enabled: true,
      }));
      pingTimer = window.setInterval(() => {
        if (socket.readyState === WebSocket.OPEN) socket.send("PING");
      }, 10000);
    };

    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || event.data === "PONG") return;
      try {
        const payload = JSON.parse(event.data);
        const messages = Array.isArray(payload) ? payload : [payload];
        for (const message of messages) applyMarketWsMessage(message, subscribedIds, setLiveQuotes);
      } catch {
        // Ignore non-JSON heartbeat payloads.
      }
    };

    socket.onerror = () => setWsState("error");
    socket.onclose = () => {
      if (!closed) setWsState("closed");
    };

    onCleanup(() => {
      closed = true;
      window.clearInterval(pingTimer);
      socket.close();
    });
  });

  return { liveQuotes, wsState };
}

function retainSubscribedQuotes(
  ids: string[],
  setLiveQuotes: (fn: (current: Map<string, LiveQuote>) => Map<string, LiveQuote>) => void,
) {
  setLiveQuotes((currentQuotes) => {
    const next = new Map<string, LiveQuote>();
    for (const id of ids) {
      const quote = currentQuotes.get(id);
      if (quote) next.set(id, quote);
    }
    return next;
  });
}
