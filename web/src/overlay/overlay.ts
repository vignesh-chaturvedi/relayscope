// The stream overlay: a transparent page for an OBS browser source that shows an alert for
// each new donation to one config. It has no framework, so it loads fast inside OBS.

import type { Donation } from "../api";
import { formatAmount, shortAddress, tokenName } from "../format";
import { followDonations } from "../live";
import { cleanMessage, readOptions } from "./options";
import "./overlay.css";

interface Alert {
  id: string;
  test: boolean;
  amount: string;
  from: string;
  message: string;
  blockTime: number | null;
  receivedAt: number;
}

interface Timing {
  signature: string;
  slot: number;
  blockTime: number | null;
  receivedAt: number;
  shownAt: number | null;
}

const options = readOptions(new URLSearchParams(location.search));
const debug = new URLSearchParams(location.search).has("debug");
const config = decodeURIComponent(location.pathname.split("/").filter(Boolean)[1] ?? "");
const stage = document.getElementById("stage")!;
document.documentElement.dataset.theme = options.theme;

// Exposed with ?debug=1, to measure how long a donation takes to reach the screen.
const timings: Timing[] = [];
if (debug) (window as unknown as { relayscope: unknown }).relayscope = { timings };

const queue: Alert[] = [];
const MAX_QUEUE = 25;
const seen = new Set<string>();
let showing = false;

function amountText(d: Donation): string {
  // Trailing zeros add nothing on stream: "0.5 SOL", not "0.5000 SOL".
  const n = formatAmount(d.amount);
  const trimmed = n.includes(".") ? n.replace(/0+$/, "").replace(/\.$/, "") : n;
  return `${trimmed} ${tokenName(d.amount)}`;
}

function fromText(d: Donation): string {
  if (d.kind === "bridge") return "bridged from another chain";
  if (d.caller) return `through ${d.caller}`;
  if (d.invoked_by) return "through another program";
  if (d.donor) return `from ${shortAddress(d.donor)}`;
  return "";
}

function remember(id: string): boolean {
  if (seen.has(id)) return false;
  seen.add(id);
  if (seen.size > 500) seen.delete(seen.values().next().value!);
  return true;
}

function onDonation(d: Donation) {
  if (!remember(`${d.signature}:${d.ordinal}`)) return;
  if (!options.automatic && d.invoked_by) return;
  const receivedAt = Date.now() / 1000;
  if (debug) timings.push({ signature: d.signature, slot: d.slot, blockTime: d.block_time, receivedAt, shownAt: null });
  enqueue({
    id: d.signature,
    test: false,
    amount: amountText(d),
    from: fromText(d),
    // A program that donates writes its own data into the message (pump.fun's fee program
    // writes two addresses), so only messages from people donating directly are shown.
    message: options.messages && d.message && !d.invoked_by ? cleanMessage(d.message, options.length) : "",
    blockTime: d.block_time,
    receivedAt,
  });
}

function onTest() {
  enqueue({
    id: `test-${Date.now()}`,
    test: true,
    amount: "0.25 SOL",
    from: "from a test donor",
    message: options.messages ? cleanMessage("This is a test alert. Real donations look like this.", options.length) : "",
    blockTime: null,
    receivedAt: Date.now() / 1000,
  });
}

function enqueue(alert: Alert) {
  queue.push(alert);
  if (queue.length > MAX_QUEUE) queue.splice(0, queue.length - MAX_QUEUE);
  if (!showing) void showNext();
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function paragraph(className: string, text: string): HTMLParagraphElement {
  const p = document.createElement("p");
  p.className = className;
  p.textContent = text;
  return p;
}

async function showNext() {
  const alert = queue.shift();
  if (!alert) {
    showing = false;
    return;
  }
  showing = true;
  const card = document.createElement("div");
  card.className = "alert";
  card.setAttribute("role", "status");
  card.append(paragraph("alert-label", alert.test ? "Test alert" : "New donation"), paragraph("alert-amount", alert.amount));
  if (alert.from) card.append(paragraph("alert-from", alert.from));
  if (alert.message) card.append(paragraph("alert-message", `“${alert.message}”`));
  stage.replaceChildren(card);
  // Read layout once so the start state applies before the transition. Animation frames would
  // also work, but a hidden page never gets one.
  void card.offsetWidth;
  card.classList.add("is-in");
  if (debug) {
    const t = timings.find((x) => x.signature === alert.id && x.shownAt === null);
    if (t) t.shownAt = Date.now() / 1000;
    const lag = alert.blockTime ? (Date.now() / 1000 - alert.blockTime).toFixed(1) : "?";
    console.info(`relayscope overlay: showing ${alert.amount}, ${lag} s after its block`);
  }
  // When donations arrive faster than they can be shown, show each for less time.
  const seconds = queue.length > 3 ? Math.max(3, options.seconds / 2) : options.seconds;
  await sleep(seconds * 1000);
  card.classList.remove("is-in");
  card.classList.add("is-out");
  await sleep(260);
  card.remove();
  await sleep(400);
  void showNext();
}

function status(text: string) {
  if (!debug) return;
  let el = document.getElementById("status");
  if (!el) {
    el = document.createElement("p");
    el.id = "status";
    document.body.append(el);
  }
  el.textContent = text;
}

function problem(text: string) {
  // Shown even without ?debug=1, since a broken URL should be visible while setting up a scene.
  const card = document.createElement("div");
  card.className = "alert is-in";
  card.append(paragraph("alert-label", "Relayscope overlay"), paragraph("alert-from", text));
  stage.replaceChildren(card);
}

function connect() {
  followDonations(
    { config, key: options.key },
    {
      donation: onDonation,
      test: onTest,
      status: (connected) =>
        status(connected ? `Connected. Waiting for donations to ${shortAddress(config, 6)}.` : "Connection lost. Reconnecting."),
    },
  );
}

// A config ID is 32 bytes, in hex or base58.
const CONFIG_ID = /^(?:0x)?[0-9a-f]{64}$|^[1-9A-HJ-NP-Za-km-z]{32,44}$/i;

if (!config) {
  problem("Add a config ID to the URL: /overlay/<config ID>.");
} else if (!CONFIG_ID.test(config)) {
  problem("This URL has no valid config ID. Copy it again from the setup page.");
} else {
  connect();
}
