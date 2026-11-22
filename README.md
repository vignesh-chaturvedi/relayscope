# Relayscope

A real-time tracker for the Donation Relay program on Solana (`RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu`).

Relayscope follows every donation from the donor's transaction to the payout, checks that every epoch balances, and gives streamers and integrators a live feed, an OBS overlay and signed webhooks.

An independent, unofficial project.

**Status:** phase 1 of 7 is done: a decoder for every instruction, event and account in the program. See the [build plan](docs/BUILD_PLAN.md).

## Try the decoder

Requires Rust 1.88 or newer.

```bash
# Decode everything the relay did in a transaction
cargo run -p relayscope -- tx 3BpzeRQe9zWQrLxXzHTa7ubBdA91zLxp3WyTHcfMcMKc9LAJfghJes23JGTK46c4AhiwbjXcz7Yad3CfrgkuKqcn

# The same, as JSON
cargo run -p relayscope -- tx <signature> --json

# Check that the on-chain IDL still matches the pinned copy
cargo run -p relayscope -- idl check
```

Set `RELAYSCOPE_RPC_URL` to use your own RPC endpoint. The default is Solana's public mainnet endpoint, which is rate limited.

## How decoding is checked

- **Against the IDL.** The program's IDL is pinned in [`idl/`](idl/) with its SHA-256. A test compares every Rust type with it, field by field: 15 instructions, 7 events, 7 accounts, 8 shared types and 34 errors. `relayscope idl check` compares the pinned copy with the chain.
- **Against real transactions.** 17 mainnet transactions in [`fixtures/`](fixtures/) decode to golden files in [`fixtures/decoded/`](fixtures/decoded/), and hand-written tests assert the values that matter: amounts, PDAs, which program made the call, and that every payout line arrived in its recipient's token account.
- **Strictly.** An unknown discriminator, or data that does not match the layout exactly, is an error, so a program upgrade cannot be silently misread.

## License

MIT
