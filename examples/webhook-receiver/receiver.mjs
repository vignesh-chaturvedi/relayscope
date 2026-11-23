// A minimal receiver for Relayscope webhooks that checks every signature. Node 18 or newer,
// no dependencies.
//
//   RELAYSCOPE_WEBHOOK_SECRET=whsec_... node examples/webhook-receiver/receiver.mjs
//   node examples/webhook-receiver/receiver.mjs --self-test
//
// It listens on 127.0.0.1:8791 (set PORT to change it), verifies each request as the Standard
// Webhooks spec describes, answers 204, skips event IDs it has already handled and prints
// each event. The spec's own libraries (npm `standardwebhooks`, PyPI `standardwebhooks` and
// others) verify the same requests.

import { createHmac, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";

/** Requests signed longer ago than this, or this far in the future, are refused. */
const TOLERANCE_SECONDS = 5 * 60;

export function keyFrom(secret) {
  if (!secret?.startsWith("whsec_")) throw new Error("the secret must start with whsec_");
  return Buffer.from(secret.slice("whsec_".length), "base64");
}

/** Returns null if the request is genuine, or the reason it is not. */
export function verify(key, headers, body, now = Math.floor(Date.now() / 1000)) {
  const id = headers["webhook-id"];
  const timestamp = headers["webhook-timestamp"];
  const signatures = headers["webhook-signature"];
  if (!id || !timestamp || !signatures) return "missing webhook-id, webhook-timestamp or webhook-signature";
  const sent = Number(timestamp);
  if (!Number.isInteger(sent) || Math.abs(now - sent) > TOLERANCE_SECONDS) return "timestamp too old or in the future";
  const expected = createHmac("sha256", key).update(`${id}.${timestamp}.`).update(body).digest();
  // The header can hold several space-separated signatures, during a secret rotation.
  for (const entry of String(signatures).split(" ")) {
    const [version, value] = entry.split(",");
    if (version !== "v1" || !value) continue;
    const given = Buffer.from(value, "base64");
    if (given.length === expected.length && timingSafeEqual(given, expected)) return null;
  }
  return "no signature matches";
}

function describe(event) {
  const d = event.data;
  const amount = (a) => (a ? `${a.raw} base units of ${a.symbol ?? a.mint}` : "?");
  switch (event.type) {
    case "donation.made":
      return `${amount(d.amount)} to config ${d.pool.config_id.slice(0, 10)}…, epoch ${d.pool.epoch}, tx ${d.signature}`;
    case "epoch.closed":
      return `epoch ${d.pool.epoch} closed with ${amount(d.gross)} from ${d.donation_count} donations`;
    case "epoch.settled":
      return `epoch ${d.pool.epoch} paid ${amount(d.payout.settled)} in ${d.payout.lines.length} lines`;
    case "check.failed":
      return `${d.check} (${d.name}) failed on epoch ${d.pool.epoch}: ${d.detail}`;
    default:
      return "unknown event type";
  }
}

function selfTest() {
  // The example from the Standard Webhooks spec.
  const key = keyFrom("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw");
  const headers = {
    "webhook-id": "msg_p5jXN8AQM9LWM0D4loKWxJek",
    "webhook-timestamp": "1614265330",
    "webhook-signature": "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
  };
  const body = Buffer.from('{"test": 2432232314}');
  const checks = [
    ["the spec's example verifies", verify(key, headers, body, 1614265330) === null],
    ["a changed body is refused", verify(key, headers, Buffer.from('{"test": 1}'), 1614265330) !== null],
    ["an old request is refused", verify(key, headers, body, 1614265330 + 3600) !== null],
    ["a wrong key is refused", verify(keyFrom("whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"), headers, body, 1614265330) !== null],
  ];
  for (const [name, ok] of checks) console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
  process.exit(checks.every(([, ok]) => ok) ? 0 : 1);
}

function serve() {
  let key;
  try {
    key = keyFrom(process.env.RELAYSCOPE_WEBHOOK_SECRET);
  } catch {
    console.error("Set RELAYSCOPE_WEBHOOK_SECRET to the endpoint's whsec_ secret.");
    process.exit(1);
  }
  const port = Number(process.env.PORT ?? 8791);
  // Event IDs already handled. A real receiver keeps these in its database, since a retry
  // after a timeout can deliver the same event twice.
  const handled = new Set();

  createServer((req, res) => {
    if (req.method !== "POST") {
      res.writeHead(405, { allow: "POST" }).end();
      return;
    }
    const chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > 1_000_000) req.destroy();
      else chunks.push(chunk);
    });
    req.on("end", () => {
      // Verify the raw bytes: parsing and re-serializing JSON would change them.
      const body = Buffer.concat(chunks);
      const problem = verify(key, req.headers, body);
      if (problem) {
        console.log(`refused a request: ${problem}`);
        res.writeHead(401).end();
        return;
      }
      // Answer quickly, then do any slow work. Relayscope waits 15 seconds at most.
      res.writeHead(204).end();
      const id = req.headers["webhook-id"];
      if (handled.has(id)) {
        console.log(`verified ${id} again, already handled`);
        return;
      }
      handled.add(id);
      const event = JSON.parse(body);
      console.log(`verified ${event.type} ${id}: ${describe(event)}`);
    });
  }).listen(port, "127.0.0.1", () => console.log(`listening on http://127.0.0.1:${port}/`));
}

if (process.argv.includes("--self-test")) selfTest();
else serve();
