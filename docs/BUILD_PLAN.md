# Relayscope build plan

Relayscope is an open-source, real-time tracker for the Donation Relay program on Solana. It follows every donation from the donor's transaction to the payout, checks that every epoch balances, and gives streamers and integrators a live feed, an OBS overlay and signed webhooks.

Relayscope is an independent, unofficial project.

Work happens in phases. Each phase ends with a commit and a push, and each phase has a "done when" test that must pass first.

## Goals

1. **A receipt for every donation.** One page per donation: donor transaction, epoch, swap, payout, with the beneficiary amount, fee and tip in exact units.
2. **Every closed epoch reconciled.** The checks are plain arithmetic over on-chain data, and anyone can rerun them with one command.
3. **Nothing missed.** Live ingestion is gap-aware and is checked against RPC, so a missing donation is caught.
4. **Useful to integrators.** A live feed, an OBS browser-source overlay and signed webhooks.

## Scope rules

- **One program.** Relayscope indexes only `RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu`. It follows funds up to the payout address the program pays. Anything after that address (custody, conversion to USD, bank payout) is off-chain and out of scope.
- **The chain is the source of truth.** Every amount on a page comes from a transaction. Labels from the operator's public API, such as charity names and stored config fees, are shown as labels and never used as proof.
- **Neutral wording.** A check passes or fails, and it shows its numbers. Relayscope makes no claims beyond the arithmetic.
- **No operator branding.** No logo and no lookalike design.

## What the program does

Facts read from the program's on-chain Anchor IDL on 22 November 2026:

| Item | Value |
| --- | --- |
| Program ID | `RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu` |
| IDL account | `6mCmANHqUcqWCLAUtB3DFJSDv14yWPq4wXUzDhdMj3Ge` |
| IDL | `donation_relay` 0.6.0, Anchor IDL spec 0.1.0 |
| Size | 15 instructions, 7 events, 7 account types, 34 errors |
| Audit | Halborn, according to the program's documentation |

### Lifecycle of a donation

Funds are pooled per `(config_id, mint)` and processed in numbered epochs.

1. **Donate.** A donor calls one of five `donate_*` instructions. An EVM donation arrives through Relay.link and the relayer admits it with `admit_bridge_payment_v1`. Either way, funds land in the `debouncer_v1` vault for `(config_id, mint)`. The program emits `DonationMadeV1Event` or `BridgeDonationAdmittedV1Event` with `config_id`, `mint`, `epoch`, `gross_amount`, `tip`, plus `message` and `credited_to` for direct donations.
2. **Close.** The relayer calls `close_donation_epoch_v1`. The epoch's funds move into a `DistributionV1` account, and `DonationEpochClosedV1Event` reports `gross_epoch_amount`.
3. **Swap.** For epochs not already in USDC, `just_swap_v1` swaps through Jupiter. `DonationEpochJustSwappedV1Event` reports `swap_output_amount` in the settlement mint (USDC), and a `SwapReceiptV1` account records it.
4. **Pay.** `distribute_exact_after_just_swapping_v1` or `distribute_exact_non_swap_output_mint_v1` pays out. `DonationEpochProcessedExactV1Event` reports `settled_amount` and a list of transfers, each typed `Beneficiary`, `Fee` or `Tip`.

Older instructions are still in the program and in its history: `distribute_after_just_swapping_v1`, `distribute_non_swap_output_mint_v1` and `swap_then_distribute_v1`. They pay by basis points and emit `DonationEpochProcessedV1Event`. Relayscope decodes both generations.

### How events are emitted

Events use Anchor's event CPI: the program invokes itself with the tag `e445a52e51cb9a1d`, followed by the event discriminator and the Borsh-encoded event. Events therefore live in a transaction's **inner instructions**. Relayscope reads events from inner instructions wherever they appear, and never relies on top-level instructions alone.

## What mainnet traffic shows

From a sample taken on 22 November 2026:

- The program processed about 1,000 transactions in about 43 hours. Most of them are relayer work: closes, swaps and payouts.
- In a sample of 134 recent transactions, 37 donation events appeared. 28 of them came through CPI from `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ`, pump.fun's fee program, which logs `CrankDonationFeePda` when it pays Charity Coin fees into the relay. The other 9 were direct calls.
- In an even sample of 562 transactions across the whole history, 205 of 209 donations were `donate_pubkey_config_id_with_payer_v1` calls from that fee program. Older bps payouts outnumber exact payouts 114 to 5, so reconciliation must handle both generations.
- One settled SOL epoch paid 160.251464 USDC: 128.201171 to the beneficiary and 32.050293 as fee. That fee is exactly 20.00%, matching the 2,000 bps API fee in the program's documentation.
- Full history on 22 November 2026: 242,765 transactions since 18 February 2026, 17,530 of them failed. About 99% came after Charity Coins launched on 29 April 2026. At this size, the backfill needs a paid RPC.

## Architecture

```
RPC WebSocket (logsSubscribe), or gapless over Solami gRPC -----+
Solana RPC (backfill and verification) -------------------------+
                                                                |
                                                                v
                             relayscope-core: decode, model, reconcile
                                                                |
                                                                v
                                                     SQLite (WAL mode)
                                                                |
                                                                v
            relayscope server (axum): REST API, live feed (SSE), webhooks, static web
                                                                |
                                                                v
            web (Vite, React, TypeScript): overview, receipts, epochs, configs, overlay
```

### Repository layout

```
relayscope/
  Cargo.toml                 workspace
  crates/
    relayscope-core/         decoding, data model, reconciliation math. No network, heavily tested.
    relayscope/              the binary: backfill, live, verify, serve, idl subcommands
  idl/
    donation_relay.json      pinned copy of the on-chain IDL, with its SHA-256
  fixtures/                  real mainnet transactions, as JSON-RPC returns them
    decoded/                 what each one must decode to (golden files)
  web/                       Vite + React + TypeScript front end
  docs/
    BUILD_PLAN.md            this file
    METHODOLOGY.md           every check, with its formula
    API.md                   REST, SSE and webhook reference
```

### Technology

| Layer | Choice | Why |
| --- | --- | --- |
| Language | Rust 2024 edition, tokio | Same stack as gapless and hyperkeeper |
| Decoding | `borsh` types written from the pinned IDL, checked against it by a test | No code generator in the build, and a failing test when the IDL changes |
| Live data | A `logsSubscribe` WebSocket on any RPC, or `gapless` over Solami's Yellowstone gRPC | The WebSocket works on a free RPC key; gapless replays gaps itself but needs a Solami plan with gRPC. Either way, an RPC repair pass fills anything missed |
| Backfill | Solana JSON-RPC `getSignaturesForAddress` and `getTransaction` | Complete history from the program's first transaction |
| Storage | SQLite through `rusqlite` (bundled), WAL mode | One file, no service to run, plenty for this volume |
| Server | axum 0.8 | REST, server-sent events and static files from one binary |
| Front end | Vite, React, TypeScript, pnpm | Built to static files the server ships |
| Deploy | One Docker image on an always-on host | No cold starts for anyone clicking a link |

## Data model

| Table | Holds |
| --- | --- |
| `transactions` | signature, slot, block time, error, how it arrived (backfill, live or replay) |
| `donations` | signature, event index, kind (direct or bridge), `config_id`, mint, epoch, gross amount, tip, message, `credited_to`, calling program |
| `epochs` | `(config_id, mint, epoch)`, close signature, gross epoch amount, swap signature, swap output, settlement mint, payout signature, settled amount, state |
| `transfers` | payout signature, position, share type (`Beneficiary`, `Fee`, `Tip`), address, amount, basis points for legacy payouts |
| `token_deltas` | per transaction and token account: balance before and after, read from transaction metadata |
| `labels` | cached operator API data per config: beneficiary names, stored fee bps, fetch time |
| `checks` | one row per check per epoch: name, result, expected, actual, detail |
| `cursor` | backfill and live positions, so every process can resume |

All writes are idempotent: re-processing a transaction never creates duplicates.

## Checks

Checks run only on finalized data. Each one stores its expected and actual values, so a failure is self-explanatory.

| ID | Check | Rule |
| --- | --- | --- |
| C1 | Epoch inflow | The sum of `gross_amount` over every donation and bridge admission in `(config, mint, epoch)` equals `gross_epoch_amount` in the close event. |
| C2 | Swap output | `swap_output_amount` in the swap event equals the settlement-mint balance increase of the output token account in the same transaction. |
| C3 | Settlement total | `settled_amount` equals the sum of transfer amounts. It also equals `swap_output_amount` when the epoch was swapped, or `gross_epoch_amount` when it was not. |
| C4 | Transfers landed | Every transfer in the payout event matches a token balance increase of that amount for that address in the same transaction. |
| C5 | Fee rate | The fee is one known rate applied to the whole epoch, within one unit of rounding per donation, or a split of the epoch's donations between two known rates that reproduces it. Phase 3 settled the rule from the history; comparing with each config's stored fee waits for the API labels in Phase 5. |
| C6 | Tip | Tips declared by donors match `Tip` lines, in proportion to the settled amount, within one unit per donation. |
| C7 | Pending time | Time from each donation to its epoch's payout. Open epochs older than a threshold are listed with their age and gross value. Very small epochs can wait by design, because the relayer batches until an epoch is worth at least 1 USDC. |
| C8 | Completeness | For every closed time window, the set of indexed signatures equals the set RPC returns for the program. |

### Per-donation receipt math

The program settles per epoch, not per donation. A donation's share is shown as an estimate and labelled that way:

- `share = gross_amount / gross_epoch_amount`
- `estimated USDC for this donation = floor(settled_amount * share)`
- Each beneficiary, fee and tip line is that transfer's amount times `share`, rounded down.

A property test checks that the estimates for all donations in an epoch never add up to more than what was settled.

## Phases

Each phase ends with `cargo fmt`, `cargo clippy -D warnings`, the tests, then a commit and a push. Commit messages are a single imperative line with no trailers.

### Phase 0: plan

- This document, a short README and the MIT license.
- **Done when:** the plan is pushed.
- **Commit:** `Add the Relayscope build plan`

### Phase 1: decoder

- Pin the IDL in `idl/donation_relay.json` with its SHA-256. Add `relayscope idl fetch` and `relayscope idl check`, which derive the IDL account address, download it, decompress it and compare it with the pinned copy.
- Write Borsh types for all 7 events, the 15 instructions' arguments and the 7 account types.
- Read events from inner instructions by tag and discriminator, and record which program made the call.
- Save about 25 real transactions as fixtures. Cover every event, every instruction, native SOL and SPL donations, CPI donations from pump.fun's fee program, a bridge admission, a legacy bps payout and a failed transaction.
- Tests: golden decodes of every fixture, a test that every IDL event and instruction has a decoder, and discriminators computed from the IDL.
- **Done when:** every fixture decodes to the expected values and `relayscope idl check` matches mainnet.
- **Commit:** `Decode every Donation Relay instruction and event`

**Status: done.** 16 real transactions are fixtures (18 after phase 2), covering 12 of the 15 instructions and 6 of the 7 events. Three never appeared in an even sample of the history: `donate_native_v1`, `donate_pubkey_config_id_v1` and `recover_bridge_payment_v1`, with its `BridgePaymentRecoveredV1Event`. Round-trip tests cover them until a real sample turns up. What the phase found:

- The program's first transactions are Anchor's built-in IDL management instructions. They have their own tag and are recorded separately, not decoded as relay instructions.
- On launch day, pump.fun's fee program rejected calls because it expected a different relay program, `DRLYxueWz6iymdsaRCER6iv6v9zL7gFWANwDL2V5VUx1`. That program is deployed on mainnet with about 900 transactions since January 2026. Phase 2 decides whether it belongs in scope.
- In the bridged USDC epoch fixture, the fee is 55,799 on 1,860,005 settled: one unit below 3% of the total. That fits a 3% fee taken per donation and rounded down. Phase 3 tests that rule across the history.

### Phase 2: backfill and storage

- SQLite schema with migrations.
- `relayscope backfill`: pages `getSignaturesForAddress` back to the program's first transaction, fetches every transaction with bounded concurrency and retries, and writes idempotently. It can stop and resume at any point.
- Uses any RPC URL that keeps full history, with a shared rate limit. Solami's RPC turned out to keep only a short recent window, so it is kept for live streaming in Phase 4. Solana's public endpoint is archival, but a trial settled at about 0.3 transactions a second under throttling, more than nine days for the full history, so the full run needs an archival provider. The backfill resumes after any stop, so any endpoint can take over at any point.
- `relayscope stats`: counts of transactions, donations, epochs and transfers, plus volume per mint.
- **Done when:** the full history is indexed, and the indexed signature count equals the count RPC reports.
- **Commit:** `Backfill the full program history into SQLite`

What the phase decided:

- **Raw transactions are kept**, compressed with zstd, next to what they decode to. `relayscope reindex` rebuilds every decoded table from them after a decoder fix, with no refetching.
- **Decode failures do not stop the backfill.** Each one is recorded with its error and the raw transaction, the run exits with code 2, and `relayscope stats` lists them. Nothing is silently skipped.
- **Effects are stored only for transactions that succeeded.** Failed transactions keep their instructions for diagnosis, but no donations, closes, swaps or payouts.
- **`DRLYxueWz6iymdsaRCER6iv6v9zL7gFWANwDL2V5VUx1` is left out.** Its on-chain IDL is the same program (same name, version, instructions, events and types). Its history is about 900 transactions since January 2026, mostly from two wallets, including IDL uploads. That looks like a test deployment, not the public donation flow. The decoder could index it by changing one constant.
- **Unreadable relay calls in failed transactions are recorded, not rejected.** The backfill found 10: nine transactions from one wallet on 28 July 2026 that called about 30 unrelated programs with empty data, and one call with an instruction the program does not have. All failed, so none changed anything. Successful transactions stay strictly decoded.

**Status: done** on 23 November 2026. The backfill ran on a Linux server through Helius's free plan at about 4 transactions a second.

- 243,023 transactions from 18 February to 23 November 2026, every listed signature fetched and stored, 0 decode failures.
- The stored set matches an independent listing made the day before by different code: 242,765 signatures up to the same slot, none missing on either side.
- 78,704 donations: 76,081 through pump.fun's fee program, 2,620 direct calls, 3 through another program and 12 bridged from EVM chains, to 7,778 configs. 110,522 SOL, 57,930 USDC and about 20 other tokens.
- 48,716 epochs closed, 48,263 swapped and 48,715 paid out: 46,091 by basis points and 2,624 in exact amounts.
- Payout lines record 8,694,902 USDC to beneficiaries.
- The database is 2.9 GB, about two thirds of it compressed raw transactions.

### Phase 3: reconciliation

- Implement C1 to C8 in `relayscope-core` as pure functions over stored rows.
- `relayscope verify`: runs every check over every finalized epoch and prints a summary with each failure's numbers.
- Investigate every failure and classify it: either a decoding bug (fix it) or a real mismatch (record it in `docs/METHODOLOGY.md` with the transaction signatures).
- Settle the fee and tip rules from the data, then decide whether C5 and C6 become checks.
- **Done when:** every finalized epoch in history has a result for every check, and no failure is unexplained.
- **Commit:** `Reconcile every closed epoch`

**Status: done** on 23 November 2026. Every check ran on all 49,159 epochs and none failed: all 48,716 closed epochs add up, all 48,263 swaps delivered what they reported, and all 48,715 payouts add up and landed with their recipients. C5 and C6 became checks. Every paid epoch was charged a single known rate (3%, 5%, 10%, 15% or 20%), except 27 that span a rate change; for each of those, Relayscope found the split of donations between two known rates that reproduces the fee. One epoch has been closed and unpaid since 29 July 2026, and 443 small epochs are open. The details, with signatures, are in [`METHODOLOGY.md`](METHODOLOGY.md).

### Phase 4: live ingestion

- Follow the program at confirmed commitment through a `logsSubscribe` WebSocket (the default, which works on a free Helius key) or through gapless over Solami's Yellowstone gRPC (`--source gapless`, which needs a Solami plan with gRPC access; the one available had none).
- Store each new transaction as provisional, then re-fetch it at finalized and rewrite it.
- Every 10 minutes, list the newest signatures from RPC and fetch anything the source missed, then run every check. If the source is down for over a minute, do this every minute instead. Every repair is logged.
- Kill test: cut the source during live traffic, hold it down, and once it is back, look up every signature RPC lists for the window in the store.
- **Done when:** a recorded kill test shows the recovered window verified against RPC with zero missing signatures.
- **Commit:** `Follow the program live, with repair and a kill test`

**Status: done** on 23 November 2026. `relayscope live` runs on a free Helius WebSocket. The Solami key had no gRPC access, so `--source gapless` is built and tested but has not been run against mainnet. Every streamed transaction is converted to the RPC format and decoded by the backfill's code; a test proves all 18 fixtures decode identically by both paths. In the [kill test](kill-tests/2026-11-23-websocket.md), the WebSocket was cut for 24 minutes while 5 transactions landed. The repair pass stored all 5 (one during the outage, four on reconnecting), nothing was missing, and transactions that arrived live reached the store about 1 second after their block. The live loop's scheduled checks reported 0 failures throughout.

### Phase 5: API and web

- REST API under `/api/v1`: overview, donation by signature, epoch, config, address search, checks.
- Live feed over server-sent events at `/api/v1/stream`, with an optional config filter.
- Labels from the operator's public endpoints (`GET /v1/configs/{configId}` and charity lookups), cached and rate limited. Relayscope works when the API is down. Pages then show addresses without names.
- Web pages:
  - **Overview:** live feed, totals per mint, settled USDC, open epochs, check status.
  - **Receipt** `/tx/:signature`: the journey from donation to close to swap to payout, with amounts, explorer links and the checks that cover it.
  - **Epoch** `/epoch/:config/:mint/:epoch`: every donation in the epoch and every check.
  - **Config** `/config/:id`: beneficiaries, epochs, totals, effective fee.
  - **Methodology** `/checks`: every check and its formula.
- Works on phones, with light and dark themes.
- **Done when:** any donation signature from mainnet opens a complete receipt page, with server time under 300 ms.
- **Commit:** `Serve receipts, epochs and configs over HTTP and the web`

### Phase 6: overlay and webhooks

- **OBS overlay** at `/overlay/:configId`: transparent background, an alert for each new donation with amount, token and message. Safe defaults: links are removed from messages, length is capped, and messages can be turned off. Includes a test-alert button for setting up a scene.
- **Webhooks** for `donation.made`, `epoch.closed`, `epoch.settled` and `check.failed`. Each one is signed with HMAC-SHA256 over the timestamp and body, has an event ID for idempotency, and is retried with backoff for up to 24 hours. Subscriptions come from a config file in this version.
- A sample receiver in `examples/` that verifies signatures.
- **Done when:** an OBS scene shows a real mainnet donation within 3 seconds of confirmation, and the sample receiver verifies a delivered webhook.
- **Commit:** `Add the OBS overlay and signed webhooks`

### Phase 7: ship

- Multi-stage Dockerfile: Rust release build plus the built web assets in one small image.
- Deploy on an always-on host. A link must answer in under a second, with no cold start.
- CI: format, clippy, tests, a web build, and a weekly `relayscope idl check` that fails if the program's IDL changes.
- README with a GIF, a quickstart (`docker compose up`), the methodology and the API reference.
- A 60-second demo video: a live donation arriving in the overlay, then its receipt page and checks.
- **Done when:** the public URL is live, CI is green and the demo is recorded.
- **Commit:** `Ship the Docker image, CI and deployment`

## Timeline

| Phase | Estimate |
| --- | --- |
| 1. Decoder | 1 day |
| 2. Backfill and storage | 1 day |
| 3. Reconciliation | 1.5 days |
| 4. Live ingestion | 1 day |
| 5. API and web | 2 days |
| 6. Overlay and webhooks | 1 day |
| 7. Ship | 1 day |
| **Total** | **about 8.5 days** |

## Risks and open questions

| Risk | Plan |
| --- | --- |
| The program is upgraded and the IDL changes | Weekly IDL check in CI. Decoders are keyed by event name and fail loudly on unknown discriminators. |
| Legacy instructions in old history | Decode both payout generations from Phase 1. Fixtures include at least one legacy payout. |
| Public RPC rate limits during backfill | A shared rate limit, retries with backoff, and a resumable queue. Any archival endpoint can be swapped in with `RELAYSCOPE_RPC_URL`. |
| The live source drops transactions or goes down | The repair pass lists the newest signatures from RPC every 10 minutes (every minute while the source is down) and fetches anything missing. The kill test proves this end to end. |
| How fees and tips combine is not fully documented | C5 and C6 stay informational until Phase 3 settles the rule from history. |
| Donor messages can contain links or spam | The overlay strips links and caps length by default, and messages can be turned off. |
| Bridge donations start on another chain | Only the Solana leg is proven. The source transaction is shown as a label when the operator's public transfer API returns it. |
| Per-donation amounts are estimates | They are labelled as estimates. Exact amounts exist only per epoch, and the page says so. |
| A second relay program, `DRLYxueW...`, is on mainnet | Phase 2 compared its IDL and sampled its history: the same program, used like a test deployment. Left out and documented. |

## Commit conventions

- One commit, or a small series, per phase, pushed when the phase's "done when" test passes.
- Subject line only, in the imperative, with no trailers or attribution lines.
