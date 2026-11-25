# Methodology

How Relayscope checks the Donation Relay program (`RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu`), and what the checks found when they were first run over the program's full history on 23 November 2026.

Every number on this page comes from on-chain data that anyone can re-fetch. To reproduce it:

```bash
cargo run --release -p relayscope -- backfill   # index the history
cargo run --release -p relayscope -- verify     # run every check, store the results, print the report
```

The checks are pure functions in [`crates/relayscope-core/src/reconcile.rs`](../crates/relayscope-core/src/reconcile.rs), with a test for each rule. `verify` stores one row per epoch and check in the `checks` table.

## What the checks prove, and what they cannot

Donations are pooled per `(config_id, mint)` and processed in numbered epochs: donations go in, the epoch is closed, swapped to USDC if needed, and paid out in lines typed `Beneficiary`, `Fee` or `Tip`. The checks follow each epoch through those steps.

They prove that nothing was lost or added between donation and payout, that every payout line arrived in the token accounts of its recipient, and which fee rate was charged.

They cannot see past the payout address. What happens after it (custody, conversion to USD, bank transfer) is off-chain. The checks also do not judge whether a rate is appropriate. They report which rate was charged.

## Data

| | |
| --- | --- |
| History | 18 February 2026 to 23 November 2026 |
| Transactions | 243,023, of which 17,531 failed on chain and changed nothing |
| Epochs | 49,159: 48,715 paid out, 1 closed and not paid, 443 open |
| Donations | 78,704: 76,081 through pump.fun's fee program, 2,620 direct calls, 3 through another program, 12 bridged from EVM chains |
| Paid out | 8,694,902.35 USDC to beneficiaries |

Effects are counted only for transactions that succeeded. Amounts are in base units unless a currency is named: SOL has 9 decimals and USDC has 6.

## Results

| Check | Pass | Fail | Pending | Not applicable |
| --- | ---: | ---: | ---: | ---: |
| C1 epoch inflow | 48,716 | 0 | 443 | 0 |
| C2 swap output | 48,263 | 0 | 0 | 896 |
| C3 settlement total | 48,715 | 0 | 1 | 443 |
| C4 payouts landed | 48,715 | 0 | 0 | 444 |
| C5 fee rate | 48,715 | 0 | 0 | 444 |
| C6 tips | 48,715 | 0 | 0 | 444 |
| C7 timing | 48,715 recorded | | 444 | |
| C8 completeness | pass | | | |

No check failed.

## The checks

### C1 epoch inflow

The sum of `gross_amount` over every `DonationMadeV1Event` and `BridgeDonationAdmittedV1Event` in `(config_id, mint, epoch)` equals `gross_epoch_amount` in its `DonationEpochClosedV1Event`. Exact, no tolerance. An epoch with donations and no close is pending.

All 48,716 closed epochs add up exactly.

### C2 swap output

The `swap_output_amount` in `DonationEpochJustSwappedV1Event` equals the increase of the swap's `output_token_account`, read from the transaction's token balances. Exact.

All 48,263 swaps match. The 896 epochs without a separate swap are 398 already in USDC, 54 paid by the older `swap_then_distribute_v1` (swap and payout in one instruction), and the 444 not paid out.

### C3 settlement total

- The payout lines add up to `settled_amount`.
- `settled_amount` equals the swap output for swapped epochs, or `gross_epoch_amount` for epochs already in the settlement mint. A payout made after a swap with no recorded swap fails.
- For older basis-point payouts, the shares add up to 10,000 bps, and each line is `floor(settled * bps / 10000)` or one unit more. That is how the program spreads the rounding remainder so the lines add up exactly.

All 48,715 payouts pass. In the basis-point payouts, 27,410 beneficiary lines and 19,331 fee lines carry the extra unit.

### C4 payouts landed

For every recipient address, the settlement mint held by token accounts it owns grew by exactly the sum of its lines in the payout transaction. Exact.

All 108,193 recipient and payout pairs match.

### C5 fee rate

The fee must be one known rate applied to the whole epoch. The known rates are every whole-epoch rate in the history: 300, 500, 1,000, 1,500 and 2,000 bps (3%, 5%, 10%, 15% and 20%).

- **Exact payouts:** `|fee * 10000 - settled * rate| <= donations * 10000`, that is, the fee is the rate times the settled amount within one unit per donation. Of 2,279 single-donation epochs, 2,247 round the fee to the nearest unit, 31 round down and 1 rounds up, all within that allowance.
- **Basis-point payouts:** the `Fee` line states its rate, and C3 has already checked its rounding.
- **Blended epochs:** the program's documentation says each donation is priced by its own slot and attribution address, so an epoch that spans a rate change mixes rates. Such an epoch passes only if its donations can be split between two known rates in a way that reproduces the fee within the same rounding. Relayscope tries every split, up to 20 donations, and reports the one it finds.

All 48,715 paid epochs pass. 48,688 were charged a single known rate. The other 27 span a rate change, and for each of them Relayscope found a split of the donations between two known rates that reproduces the fee. The largest has 19 donations.

### C6 tips

If no donor in the epoch declared a tip, there is no tip line. Otherwise the tip lines equal `settled * declared_tips / gross_epoch_amount`, within one unit per donation.

Three donations in the history declared a tip, each 1 bps (1,200 lamports on 0.012 SOL), and their epochs paid tips of 104, 105 and 105 USDC base units, matching. Every other epoch has no tip line.

### C7 timing

For each paid epoch, the time from its oldest donation to its payout. For each unpaid epoch, how long it has waited. This is recorded, not passed or failed.

- Paid epochs: median 39 seconds, 90% within 17.6 hours, 99% within 15.2 days, longest 209.2 days.
- **Closed and not paid out:** one epoch. Config `0x3c717c4180e4e2d26b7faae7b34eff7a29e3ac68d04234ab7327cf3ce10e729f`, mint `3RVoTazgrD5kwprTDaRjipoUV7GA2YXtPMGtZ6cfpump`, epoch 0. It holds one direct donation of 1,500,000,000,000 base units of that pump.fun token (signature `4PmRood2dMKSWw6QJaMtL8RhZk6Uhfrg533xBvKo3d37ymJqADUdvLs5wL9h41JVchAeKJFLLXHcHS9yezEjJE1C`). It was closed on 29 July 2026 (`5VQyi81TBAymgagMao5VHRca2ttaHE7UJhkJ1Be45DQbMnbSHwgPbuT1fcqUrC9ybdLhwNvs3QZUesg7QWaSVJnc`), and no swap or payout of it, successful or failed, appears since.
- **Open:** 443 epochs with 560 donations have never been closed, 423 of them for more than 7 days. They are small: 394 SOL epochs hold 2.876 SOL in all, and 43 USDC epochs hold 10.06 USDC. The program's documentation says the relayer waits until an epoch is worth at least 1 USDC before closing it, which fits.

### C8 completeness

Listing reached the program's first transaction, every listed signature was fetched and stored, and no transaction was rejected by the decoder.

243,023 signatures, 0 queued, 0 decode failures. The stored set also matches an independent listing made a day earlier by separate code: 242,765 signatures up to the same slot, none missing on either side.

## Other findings

- **Unreadable calls in failed transactions.** 10 failed transactions called the relay with data it does not understand: nine from one wallet on 28 July 2026 that called about 30 unrelated programs with empty data, and one call to an instruction the program does not have, which it rejected with Anchor error 101. They changed nothing, and are recorded as `unrecognized` instructions.
- **A second deployment.** `DRLYxueWz6iymdsaRCER6iv6v9zL7gFWANwDL2V5VUx1` runs the same program (identical IDL) with about 900 transactions, mostly from two wallets and including IDL uploads. It looks like a test deployment and is not indexed.
