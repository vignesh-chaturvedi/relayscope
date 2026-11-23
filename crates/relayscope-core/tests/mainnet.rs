//! Hand-checked facts about real mainnet transactions. The golden files catch any change; these
//! tests state what the decoded values must be, so the golden files are not trusted blindly.

use std::path::Path;

use relayscope_core::types::ShareType;
use relayscope_core::{
    Bytes32, DecodedTx, Event, Instruction, Pubkey, TxView, decode_transaction, pda,
};

const WSOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

fn load(name: &str) -> DecodedTx {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(format!("{name}.json"));
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    decode_transaction(&TxView::from_rpc_json(&raw).unwrap()).unwrap()
}

fn key(s: &str) -> Pubkey {
    s.parse().unwrap()
}

/// Net change in `mint` held by token accounts owned by `owner`.
fn owner_delta(tx: &DecodedTx, owner: Pubkey, mint: Pubkey) -> i128 {
    tx.token_balances
        .iter()
        .filter(|b| b.owner == Some(owner) && b.mint == mint)
        .map(|b| i128::from(b.post) - i128::from(b.pre))
        .sum()
}

#[test]
fn exact_payout_after_swap() {
    let tx = load("payout-exact-after-swap");
    assert!(tx.succeeded);
    assert_eq!(tx.instructions.len(), 1);
    assert_eq!(
        tx.instructions[0].instruction.name(),
        "distribute_exact_after_just_swapping_v1"
    );
    assert_eq!(tx.instructions[0].invoked_by, None);

    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    assert_eq!(event.emitted_by, Some(0));
    let Event::DonationEpochProcessedExactV1Event(paid) = &event.event else {
        panic!("wrong event")
    };
    assert_eq!(paid.mint, key(WSOL));
    assert_eq!(paid.settlement_mint, key(USDC));
    assert_eq!(paid.epoch, 2);
    assert_eq!(paid.settled_amount, 160_251_464);

    let lines: Vec<(ShareType, u64)> = paid
        .transfers
        .iter()
        .map(|t| (t.share_type, t.amount))
        .collect();
    assert_eq!(
        lines,
        [
            (ShareType::Beneficiary, 128_201_171),
            (ShareType::Fee, 32_050_293)
        ]
    );
    assert_eq!(
        paid.transfers.iter().map(|t| t.amount).sum::<u64>(),
        paid.settled_amount
    );
    // The fee is exactly 2,000 bps of what was settled.
    assert_eq!(32_050_293u128 * 10_000 / 160_251_464, 2_000);

    // Every payout line arrived in a token account owned by its address.
    for transfer in &paid.transfers {
        assert_eq!(
            owner_delta(&tx, transfer.address, key(USDC)),
            i128::from(transfer.amount)
        );
    }
}

#[test]
fn direct_native_donation() {
    let tx = load("donate-native-pubkey-direct");
    let [ix] = tx.instructions.as_slice() else {
        panic!("expected one instruction")
    };
    assert_eq!(ix.invoked_by, None, "called directly, not through CPI");
    let Instruction::DonateNativePubkeyConfigIdV1(args) = &ix.instruction else {
        panic!("wrong instruction")
    };
    assert_eq!(args.amount, 16_493_290);

    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    let Event::DonationMadeV1Event(donation) = &event.event else {
        panic!("wrong event")
    };
    let config: Bytes32 = "0x631b15a64d03c91971105885649161ef707671b98f73990d7de822138c4006f4"
        .parse()
        .unwrap();
    assert_eq!(donation.config_id, config);
    assert_eq!(
        ix.instruction.config_id(),
        Some(config),
        "pubkey config ID is the same 32 bytes"
    );
    assert_eq!(donation.gross_amount, args.amount);
    assert_eq!(donation.mint, key(WSOL));
    assert_eq!(donation.epoch, 0);
    assert!(donation.message.starts_with("Ho ho ho!"));

    // The accounts the donor passed are the program's PDAs for this pool.
    assert_eq!(
        ix.account("epoch_tracker"),
        Some(pda::epoch_tracker(&config, &key(WSOL)))
    );
    assert_eq!(
        ix.account("debouncer"),
        Some(pda::debouncer(&config, &key(WSOL)))
    );
    assert_eq!(ix.account("mint_whitelist"), Some(pda::mint_whitelist()));
    assert_eq!(ix.account("event_authority"), Some(pda::event_authority()));
    assert_eq!(ix.account("from"), Some(tx.fee_payer));
}

#[test]
fn epoch_close() {
    let tx = load("close-epoch");
    let [ix] = tx.instructions.as_slice() else {
        panic!("expected one instruction")
    };
    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    let Event::DonationEpochClosedV1Event(closed) = &event.event else {
        panic!("wrong event")
    };
    assert_eq!(closed.epoch, 62);
    assert_eq!(closed.gross_epoch_amount, 41_189_654_010);
    let Instruction::CloseDonationEpochV1(args) = &ix.instruction else {
        panic!("wrong instruction")
    };
    assert_eq!(
        (args.config_id, args.epoch),
        (closed.config_id, closed.epoch)
    );
    assert_eq!(
        ix.account("distribution"),
        Some(pda::distribution(
            &closed.config_id,
            &closed.mint,
            closed.epoch
        ))
    );
}

#[test]
fn swap_output_matches_the_token_balance_change() {
    let tx = load("just-swap");
    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    let Event::DonationEpochJustSwappedV1Event(swap) = &event.event else {
        panic!("wrong event")
    };
    assert_eq!(swap.swap_output_amount, 44_100_361);
    assert_eq!(swap.settlement_mint, key(USDC));
    let output = tx
        .token_balances
        .iter()
        .find(|b| b.account == swap.output_token_account)
        .expect("output token account changed");
    assert_eq!(output.post - output.pre, swap.swap_output_amount);
}

#[test]
fn anchor_idl_management_is_set_aside() {
    let tx = load("idl-create-account");
    assert!(tx.instructions.is_empty());
    assert!(tx.events.is_empty());
    assert_eq!(tx.idl_management.len(), 1);
    assert!(tx.touches_relay());
}

#[test]
fn mint_whitelist_upsert() {
    let tx = load("upsert-mint-whitelist");
    let [ix] = tx.instructions.as_slice() else {
        panic!("expected one instruction")
    };
    let Instruction::UpsertMintWhitelistV1(args) = &ix.instruction else {
        panic!("wrong instruction")
    };
    assert!(!args.mints.is_empty());
    assert_eq!(ix.account("mint_whitelist"), Some(pda::mint_whitelist()));
}

#[test]
fn legacy_bps_payout_adds_up() {
    let tx = load("payout-legacy-non-swap");
    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    let Event::DonationEpochProcessedV1Event(paid) = &event.event else {
        panic!("wrong event")
    };
    assert_eq!(
        paid.transfers.iter().map(|t| t.amount).sum::<u64>(),
        paid.settled_amount
    );
    assert_eq!(
        paid.transfers.iter().map(|t| u32::from(t.bps)).sum::<u32>(),
        10_000
    );
}

#[test]
fn charity_coin_donation_through_pump_fun_fees() {
    let tx = load("donate-cpi-pump-fees");
    let pump_fees = key("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
    let [ix] = tx.instructions.as_slice() else {
        panic!("expected one instruction")
    };
    assert_eq!(
        ix.instruction.name(),
        "donate_pubkey_config_id_with_payer_v1"
    );
    assert_eq!(
        ix.invoked_by,
        Some(pump_fees),
        "called through CPI by pump.fun's fee program"
    );
    assert_eq!(ix.stack_height, 2);

    let [event] = tx.events.as_slice() else {
        panic!("expected one event")
    };
    assert_eq!(
        tx.emitter(event).map(|i| i.position),
        Some(ix.position),
        "the event points at its instruction"
    );
    let Event::DonationMadeV1Event(donation) = &event.event else {
        panic!("wrong event")
    };
    assert_eq!(donation.gross_amount, 73_036_205_764);
    assert_eq!(donation.credited_to, pump_fees);
    assert_eq!(Some(donation.config_id), ix.instruction.config_id());

    // The pool's vault received exactly the donated amount.
    let vault = ix.account("debouncer_token_account").unwrap();
    let change = tx
        .token_balances
        .iter()
        .find(|b| b.account == vault)
        .unwrap();
    assert_eq!(change.post - change.pre, donation.gross_amount);
}

#[test]
fn bridged_usdc_epoch_from_admission_to_payout() {
    let admitted = load("bridge-admit");
    let closed = load("close-epoch-bridge-usdc");
    let paid = load("payout-exact-non-swap");

    let Event::BridgeDonationAdmittedV1Event(admission) = &admitted.events[0].event else {
        panic!("wrong event")
    };
    let Event::DonationEpochClosedV1Event(close) = &closed.events[0].event else {
        panic!("wrong event")
    };
    let Event::DonationEpochProcessedExactV1Event(payout) = &paid.events[0].event else {
        panic!("wrong event")
    };

    let pool = (admission.config_id, admission.mint, admission.epoch);
    assert_eq!(pool, (close.config_id, close.mint, close.epoch));
    assert_eq!(pool, (payout.config_id, payout.mint, payout.epoch));
    assert_eq!(admission.mint, key(USDC));
    assert_eq!(admission.gross_amount, 932_111);
    assert!(close.gross_epoch_amount >= admission.gross_amount);

    // Already in USDC, so nothing was swapped: everything that closed was settled.
    assert_eq!(
        paid.instructions[0].instruction.name(),
        "distribute_exact_non_swap_output_mint_v1"
    );
    assert_eq!(payout.settled_amount, close.gross_epoch_amount);
    assert_eq!(
        payout.transfers.iter().map(|t| t.amount).sum::<u64>(),
        payout.settled_amount
    );
    let lines: Vec<(ShareType, u64)> = payout
        .transfers
        .iter()
        .map(|t| (t.share_type, t.amount))
        .collect();
    assert_eq!(
        lines,
        [
            (ShareType::Beneficiary, 1_804_206),
            (ShareType::Fee, 55_799)
        ]
    );
    // 55,799 is just under 3% of 1,860,005 (which would round down to 55,800). That fits a 3%
    // fee taken per donation and rounded down; Phase 3 checks the rule across history.
    assert_eq!(1_860_005u64 * 300 / 10_000, 55_800);
}

#[test]
fn legacy_bps_payout_after_swap() {
    let tx = load("payout-legacy-after-swap");
    let [ix] = tx.instructions.as_slice() else {
        panic!("expected one instruction")
    };
    assert_eq!(ix.instruction.name(), "distribute_after_just_swapping_v1");
    let Event::DonationEpochProcessedV1Event(paid) = &tx.events[0].event else {
        panic!("wrong event")
    };
    assert_eq!(
        paid.transfers.iter().map(|t| t.amount).sum::<u64>(),
        paid.settled_amount
    );
    for transfer in &paid.transfers {
        assert_eq!(
            owner_delta(&tx, transfer.address, paid.settlement_mint),
            i128::from(transfer.amount)
        );
    }
}

#[test]
fn failed_transactions_record_no_relay_effects() {
    // Pump.fun's fee crank ran out of compute before reaching the relay.
    let out_of_compute = load("failed-pump-crank-out-of-compute");
    assert!(!out_of_compute.succeeded);
    assert!(out_of_compute.events.is_empty());

    // On launch day the fee program expected a different relay program and rejected this one.
    let launch_day = load("failed-launch-day-wrong-relay");
    assert!(!launch_day.succeeded);
    assert!(launch_day.events.is_empty());
    assert_eq!(
        launch_day.program_error, None,
        "the error came from the caller, not the relay"
    );
}

#[test]
fn bot_calls_with_empty_data_are_recorded_not_rejected() {
    // One of nine transactions from a single wallet on 2026-07-28 that called about 30
    // unrelated programs with empty data. Here the relay itself ran out of compute on it.
    let tx = load("failed-bot-empty-data");
    assert!(!tx.succeeded);
    assert!(tx.instructions.is_empty());
    assert!(tx.events.is_empty());
    let [call] = tx.unrecognized.as_slice() else {
        panic!("expected one unrecognized call")
    };
    assert_eq!(call.data_len, 0);
    assert_eq!(call.discriminator, None);
}

#[test]
fn made_up_instruction_is_recorded_not_rejected() {
    // The relay rejected it with Anchor's InstructionFallbackNotFound (error 101).
    let tx = load("failed-unknown-instruction");
    assert!(!tx.succeeded);
    let [call] = tx.unrecognized.as_slice() else {
        panic!("expected one unrecognized call")
    };
    assert_eq!(
        call.discriminator,
        Some([252, 18, 8, 187, 50, 141, 209, 182])
    );
}

#[test]
fn successful_transactions_never_have_unrecognized_calls() {
    for name in [
        "payout-exact-after-swap",
        "donate-cpi-pump-fees",
        "bridge-admit",
        "close-epoch",
    ] {
        assert!(load(name).unrecognized.is_empty(), "{name}");
    }
}
