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
- In a sample of 134 transactions, 37 donation events appeared. 28 of them came through CPI from `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ`, which appears to be pump.fun's fee program. The other 9 were direct calls. Phase 1 confirms who the calling programs are.
- One settled SOL epoch paid 160.251464 USDC: 128.201171 to the beneficiary and 32.050293 as fee. That fee is exactly 20.00%, matching the 2,000 bps API fee in the program's documentation.
- Full history: more than 220,000 transactions, reaching back past early May 2026. Phase 2 records the exact count and the date of the first transaction. At this size, the backfill needs a paid RPC.

## Architecture

```
Solami Yellowstone gRPC --> gapless (exactly-once, gap-aware) --+
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
  fixtures/                  real mainnet transactions used as golden test cases
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
| Live data | `gapless` as a git dependency, Solami Yellowstone gRPC | Exactly-once delivery, resume after drops, RPC verification |
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
| C5 | Fee rate | The effective fee is `Fee / settled` in basis points, compared with the config's stored fee. Shown as information until Phase 3 confirms how fees and tips interact across the full history, and only then promoted to a check. |
| C6 | Tip | Tips declared by donors match `Tip` transfers, pro rata to the swap when the epoch was swapped. Same promotion rule as C5. |
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

### Phase 2: backfill and storage

- SQLite schema with migrations.
- `relayscope backfill`: pages `getSignaturesForAddress` back to the program's first transaction, fetches every transaction with bounded concurrency and retries, and writes idempotently. It can stop and resume at any point.
- Uses Solami's RPC (the existing key) or any RPC URL, with a rate limit.
- `relayscope stats`: counts of transactions, donations, epochs and transfers, plus volume per mint.
- **Done when:** the full history is indexed, and the indexed signature count equals the count RPC reports.
- **Commit:** `Backfill the full program history into SQLite`

### Phase 3: reconciliation

- Implement C1 to C8 in `relayscope-core` as pure functions over stored rows.
- `relayscope verify`: runs every check over every finalized epoch and prints a summary with each failure's numbers.
- Investigate every failure and classify it: either a decoding bug (fix it) or a real mismatch (record it in `docs/METHODOLOGY.md` with the transaction signatures).
- Settle the fee and tip rules from the data, then decide whether C5 and C6 become checks.
- **Done when:** every finalized epoch in history has a result for every check, and no failure is unexplained.
- **Commit:** `Reconcile every closed epoch`

### Phase 4: live ingestion

- Subscribe to the program through gapless with Solami's Yellowstone gRPC.
- Write at confirmed commitment for the live feed. Promote to finalized, then run checks.
- Fall back to RPC polling if gRPC is unavailable, and log every fallback.
- Run C8 hourly over the last window.
- Kill test: cut the stream with gapless's controls during live traffic and show that nothing was missed.
- **Done when:** a recorded kill test shows the recovered window verified against RPC with zero missing signatures.
- **Commit:** `Stream new transactions live through gapless`

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
| Public RPC rate limits during backfill | Use Solami's RPC or another paid endpoint, with bounded concurrency and retries. |
| Solami's gRPC plan allows two streams | Relayscope uses one. The gapless handoff patch needs the second only for a few seconds after a recovery. |
| How fees and tips combine is not fully documented | C5 and C6 stay informational until Phase 3 settles the rule from history. |
| Donor messages can contain links or spam | The overlay strips links and caps length by default, and messages can be turned off. |
| Bridge donations start on another chain | Only the Solana leg is proven. The source transaction is shown as a label when the operator's public transfer API returns it. |
| Per-donation amounts are estimates | They are labelled as estimates. Exact amounts exist only per epoch, and the page says so. |

## Commit conventions

- One commit, or a small series, per phase, pushed when the phase's "done when" test passes.
- Subject line only, in the imperative, with no trailers or attribution lines.
