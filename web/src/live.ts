// The live feed of new donations over a WebSocket. It passes through proxies that hold back
// server-sent events (Cloudflare's quick tunnels do), reconnects with backoff, and on
// reconnecting asks for what it missed.

import type { Donation } from "./api";

export interface LiveHandlers {
  donation: (d: Donation) => void;
  test?: () => void;
  status?: (connected: boolean) => void;
}

/** Follows new donations, to one config or all of them. Returns a function that stops it. */
export function followDonations(filter: { config?: string; key?: string | null }, on: LiveHandlers): () => void {
  let socket: WebSocket | null = null;
  let stopped = false;
  let lastSlot: number | null = null;
  let wait = 1_000;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function open() {
    const params = new URLSearchParams();
    if (filter.config) params.set("config", filter.config);
    if (filter.key) params.set("key", filter.key);
    if (filter.config && lastSlot !== null) params.set("after", String(lastSlot));
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(`${scheme}//${location.host}/api/v1/live?${params}`);
    socket.onopen = () => {
      wait = 1_000;
      on.status?.(true);
    };
    socket.onmessage = (event) => {
      const message = JSON.parse(event.data as string) as { type: "donation"; donation: Donation } | { type: "test" };
      if (message.type === "donation") {
        lastSlot = Math.max(lastSlot ?? 0, message.donation.slot);
        on.donation(message.donation);
      } else if (message.type === "test") {
        on.test?.();
      }
    };
    socket.onclose = () => {
      on.status?.(false);
      if (stopped) return;
      timer = setTimeout(open, wait);
      wait = Math.min(wait * 2, 15_000);
    };
  }

  open();
  return () => {
    stopped = true;
    clearTimeout(timer);
    socket?.close();
  };
}
