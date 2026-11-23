# Webhooks

Relayscope can send a signed POST request to your server for each new donation, each closed and settled epoch, and each failed check. Requests follow the [Standard Webhooks](https://www.standardwebhooks.com) spec, so its libraries verify them, and so does the dependency-free [sample receiver](../examples/webhook-receiver/receiver.mjs).

## Turn them on

```bash
# 1. Make a signing secret for your endpoint
relayscope webhooks secret

# 2. List your endpoints in a TOML file (see examples/webhooks.toml), then serve with it
relayscope serve --webhooks webhooks.toml

# 3. See what was delivered, what is waiting and what failed
relayscope webhooks status
```

`relayscope serve` finds events in what `relayscope live` stores, so run both. Only what is stored after webhooks are first turned on is sent: the history is never replayed. An endpoint added to the file later gets events from then on.

## Events

| Type | Sent when | `data` holds |
| --- | --- | --- |
| `donation.made` | A donation is stored, about a second after its block | The donation as the API returns it (signature, pool, amount, message, donor, caller, slot, block time, `finalized`) and `url`, its receipt |
| `epoch.closed` | The relayer closes an epoch | `pool`, the close transaction's `signature`, `slot`, `block_time` and `finalized`, the `gross` amount closed, `donation_count`, `url` |
| `epoch.settled` | An epoch's payout is stored | `pool`, the payout transaction's `signature`, `slot`, `block_time` and `finalized`, `donation_count`, `donated`, `swap`, `payout` with every line, `url` |
| `check.failed` | Two check runs in a row find the same failure | `pool`, `check` (such as `C3`), `name`, `expected`, `actual`, `detail`, `url` |

Amounts are exact strings of base units with their mint, symbol and decimals, because JSON numbers lose precision above 2^53. `url` is set when the file has `public_url`.

A transaction the live source missed and the repair pass recovered still sends its events, only later. Checks run every 10 minutes, and a failure is sent only when the next run finds it again, so an epoch caught between two transactions (a payout stored before its swap, say) never raises a false alarm.

Donations are sent at `confirmed` commitment with `"finalized": false`, so they arrive within seconds. Relayscope rewrites the transaction once it is finalized, but does not send the event again. If you need finality, wait until the receipt API at `/api/v1/tx/{signature}` reports `"finalized": true`.

### Example

A real `donation.made` from mainnet, as the sample receiver got it (with `url` set by `public_url`):

```json
{
  "type": "donation.made",
  "timestamp": "2026-11-23T15:01:49Z",
  "data": {
    "signature": "3iQy8DMrJCpfaQ3pqRdU21fhzQmZNU73UutwxmGEJmzj5k1i72mjtBqEC842FKWVYS93UCy9Xr2WaNsodbag4Bop",
    "ordinal": 0,
    "kind": "direct",
    "pool": {
      "config_id": "0x72907798c56acb88e47179add5313b9568a4c1750d554b76eb9620de59a3fa6e",
      "mint": "So11111111111111111111111111111111111111112",
      "epoch": 8
    },
    "amount": {
      "raw": "115278519",
      "mint": "So11111111111111111111111111111111111111112",
      "symbol": "SOL",
      "decimals": 9
    },
    "tip": "0",
    "message": "MAXXib7SQAQn4ymrwhhR9qMRUcYv8BNhbQxizJn5CCk,3V5GgtZib5PdwpV7ujzzUAvNHUZjyGLYSGmWESGrAmUq",
    "credited_to": "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",
    "donor": "ChXW9NHx56TKUxCNo93ACD1yonLE4nZh2yrRMGgnn4J8",
    "invoked_by": "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",
    "caller": "pump.fun fees",
    "slot": 472985418,
    "block_time": 1795446108,
    "finalized": false,
    "url": "https://relayscope.example/tx/3iQy8DMrJCpfaQ3pqRdU21fhzQmZNU73UutwxmGEJmzj5k1i72mjtBqEC842FKWVYS93UCy9Xr2WaNsodbag4Bop"
  }
}
```

`invoked_by` is set when another program made the donation, here pump.fun's fee program. A program writes its own data into `message` (this one wrote two addresses), so treat `message` as words from a person only when `invoked_by` is null.

## Requests

Each request is a POST with a JSON body and three headers:

| Header | Value |
| --- | --- |
| `webhook-id` | The event's ID, such as `msg_5f0c…`. The same event always has the same ID, across retries and restarts. |
| `webhook-timestamp` | When this attempt was sent, in Unix seconds |
| `webhook-signature` | `v1,` and the base64 HMAC-SHA256 of `{webhook-id}.{webhook-timestamp}.{body}` |

The HMAC key is the secret after `whsec_`, decoded from base64.

## Verify a request

1. Compute the signature over the raw body bytes. Parsing and re-serializing the JSON changes them.
2. Compare it with each `v1,` entry in `webhook-signature` in constant time.
3. Refuse a `webhook-timestamp` more than 5 minutes from your clock, so a captured request cannot be replayed later.
4. Keep the IDs you have handled and skip repeats. Delivery is at least once: if your server handles an event but its answer times out, the event is sent again.
5. Answer with any 2xx status within 15 seconds, then do slow work afterwards.

```bash
# The sample receiver does all five, with no dependencies
RELAYSCOPE_WEBHOOK_SECRET=whsec_... node examples/webhook-receiver/receiver.mjs

# It also checks itself against the example in the Standard Webhooks spec
node examples/webhook-receiver/receiver.mjs --self-test
```

## Delivery and retries

- Any 2xx answer is a delivery. Anything else, a connection error or no answer within 15 seconds is a failure. Redirects are not followed.
- A failed delivery is retried after 5 seconds, 30 seconds, 2 minutes, 10 minutes, 30 minutes, 1 hour, 2 hours, 4 hours, 6 hours and 8 hours: 11 attempts over about 22 hours, then it is marked failed.
- `410 Gone` stops the retries for that event at once.
- Events can arrive out of order, especially after retries. Use `slot` to order them.
- The outbox lives in the same SQLite database, so deliveries survive a restart and continue where they stopped.

## Config file

```toml
# Optional: this site's public address, for the url field in payloads
public_url = "https://relayscope.example"

[[endpoint]]
url = "https://example.com/hooks/relayscope"
# Either the secret itself, or the name of an environment variable that holds it
secret_env = "RELAYSCOPE_WEBHOOK_SECRET"
# Optional, default all four
events = ["donation.made", "epoch.settled"]
# Optional, default every config; hex or base58
configs = ["0x4407765094cc24440154b89368fbb29d7ee629b4127758494b58725fddfd63da"]
```

Each endpoint is identified by its URL, so a URL can appear only once. Unknown fields and event names are errors, so a typo cannot silently turn a subscription off.
