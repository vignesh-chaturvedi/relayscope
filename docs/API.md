# API

`relayscope serve` answers JSON under `/api/v1`. Everything is read-only except test alerts. Amounts are objects with `raw` (exact base units, as a string, because JSON numbers lose precision above 2^53), `mint`, `symbol` (for SOL, USDC and USDT, otherwise null) and `decimals`. Times are Unix seconds. Config IDs are accepted in hex (`0x…`) or base58 and returned in hex. Errors are `{"error": "..."}` with status 400 or 404.

Every response has a `Server-Timing` header with the time spent in the server.

## Endpoints

| Endpoint | Returns |
| --- | --- |
| `GET /api/v1/health` | `{"ok": true, "indexing": {...}}`: transactions stored, first and last block time, how many are waiting to be finalized, queued or failed to decode, and whether the history is complete |
| `GET /api/v1/overview` | Totals: donation count, donated and paid to beneficiaries per mint, epochs paid, closed and open, a summary of every check, and the 25 newest donations. Cached for 10 seconds. |
| `GET /api/v1/tx/{signature}` | `transaction`: slot, block time, success, finalized, the program's instructions, the donations it made and the epochs it closed, swapped or paid. `journeys`: for each donation, its epoch, this donation's share of each payout line, and a label if one is cached. 404 if the transaction is not indexed. |
| `GET /api/v1/epoch/{config}/{mint}/{epoch}` | `epoch`: status (`open`, `closed`, `swapped` or `paid`), donation count, amount donated, up to 500 donations, the close, swap and payout with every line, and every check result. `label`. |
| `GET /api/v1/config/{config}` | `config`: donation and epoch counts, donated and paid to beneficiaries per mint, and up to 500 epochs, newest first. `label`. |
| `GET /api/v1/address/{address}` | `address`: up to 200 donations the address made or was credited with, and up to 200 payout lines it received, newest first, with totals counted. |
| `GET /api/v1/label/{config}` | `label`: beneficiary names from the operator's public API, fetched if needed. Null when `RELAYSCOPE_LABELS_URL` is unset or the API has none. Labels are never proof. |
| `GET /api/v1/search?q=` | `{"kind": "transaction" \| "config" \| "address" \| "nothing", ...}` for a signature, config ID or address |
| `GET /api/v1/checks` | `checks`: for C1 to C7, how many epochs pass, fail, are pending, informational or not applicable. `indexing`, as in health. |

[METHODOLOGY.md](METHODOLOGY.md) explains what each check proves.

```bash
curl -s localhost:8790/api/v1/search?q=3iQy8DMrJCpfaQ3pqRdU21fhzQmZNU73UutwxmGEJmzj5k1i72mjtBqEC842FKWVYS93UCy9Xr2WaNsodbag4Bop
# {"kind":"transaction","signature":"3iQy8DMr…"}
```

## Live feed

New donations are pushed about a second after their block, two ways. Both take `?config=` to follow one config, and `?key=` to also get that overlay's test alerts. A bad config ID is refused with 400 rather than ignored.

**WebSocket**, `GET /api/v1/live`. Each message is JSON: `{"type": "donation", "donation": {...}}` or `{"type": "test"}`. With `config`, `?after=<slot>` first sends up to 20 donations at or after that slot, so a client that reconnects can catch up. The server pings every 15 seconds. The web app and the overlay use this one, because it passes through proxies that hold back server-sent events, such as Cloudflare's quick tunnels.

**Server-sent events**, `GET /api/v1/stream`. Each donation is a `donation` event whose `id` is its slot; test alerts are `test` events. With `config`, a client that reconnects with `Last-Event-ID` first gets up to 20 donations it missed. Browsers' `EventSource` does this by itself.

```bash
curl -N "localhost:8790/api/v1/stream?config=0x72907798c56acb88e47179add5313b9568a4c1750d554b76eb9620de59a3fa6e"
```

A donation has `signature`, `ordinal`, `kind` (`direct` or `bridge`), `pool` (`config_id`, `mint`, `epoch`), `amount`, `tip`, `message`, `credited_to`, `donor`, `invoked_by` (the program that made the donation, if one did), `caller` (its name, if known), `slot`, `block_time` and `finalized`. When `invoked_by` is set, `message` is data that program wrote, not words from a person.

## Test alerts

`POST /api/v1/overlay/test` with `{"config": "...", "key": "..."}` sends a test alert to the overlays open with that config and key, and answers `{"overlays": n}`, how many are open. The key is 16 to 64 letters, digits, `-` or `_`; the setup page makes a random one. One test per key every two seconds; more get 429.

## Pages

`/` overview, `/tx/{signature}` receipt, `/epoch/{config}/{mint}/{epoch}`, `/config/{config}`, `/config/{config}/overlay` overlay setup, `/address/{address}`, `/checks`, and `/overlay/{config}`, the overlay itself. [The README](../README.md#show-donations-on-stream) lists the overlay's options.

## Webhooks

Signed POST requests for new donations, closed and settled epochs, and failed checks. See [WEBHOOKS.md](WEBHOOKS.md).
