//! Reconciliation: plain arithmetic over what each epoch recorded on chain.
//!
//! Every check is a pure function of an [`EpochData`], so anyone can rerun it on the same
//! rows and get the same answer. `docs/METHODOLOGY.md` explains each check and what the
//! program's full history showed when they were first run.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::types::ShareType;

/// Every whole-epoch fee rate seen in the program's history, in basis points: 3%, 5%, 10%,
/// 15% and 20%. A fee is "known" if it is one of these applied to the whole epoch.
pub const KNOWN_FEE_RATES_BPS: &[u64] = &[300, 500, 1_000, 1_500, 2_000];

const DAY: i64 = 86_400;

/// A donation or bridge admission recorded in an epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DonationRow {
    pub signature: String,
    pub gross: u64,
    pub tip: u64,
    pub block_time: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseRow {
    pub signature: String,
    pub gross_epoch: u64,
    pub block_time: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapRow {
    pub signature: String,
    pub output: u64,
    /// How much the swap's output token account actually gained in the same transaction.
    pub output_received: Option<i128>,
    pub block_time: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineRow {
    pub share_type: ShareType,
    pub address: String,
    pub amount: u64,
    /// Basis points, for older payouts that pay by share.
    pub bps: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutRow {
    pub signature: String,
    /// The instruction that paid out, such as `distribute_exact_after_just_swapping_v1`.
    pub instruction: String,
    /// `true` for exact-amount payouts, `false` for older basis-point payouts.
    pub exact: bool,
    pub settled: u64,
    pub settlement_mint: String,
    pub lines: Vec<LineRow>,
    /// Net change in the settlement mint per token-account owner, in the payout transaction.
    pub received: BTreeMap<String, i128>,
    pub block_time: Option<i64>,
}

/// Everything recorded for one `(config_id, mint, epoch)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpochData {
    pub config_id: String,
    pub mint: String,
    pub epoch: u128,
    pub donations: Vec<DonationRow>,
    pub closes: Vec<CloseRow>,
    pub swaps: Vec<SwapRow>,
    pub payouts: Vec<PayoutRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Check {
    /// C1: donations add up to the closed total.
    Inflow,
    /// C2: the swap's reported output arrived.
    SwapOutput,
    /// C3: the payout adds up, and matches what was swapped or closed.
    Settlement,
    /// C4: every payout line arrived with its recipient.
    Landed,
    /// C5: the fee is one known rate applied to the whole epoch.
    FeeRate,
    /// C6: tips declared by donors were paid as tips.
    Tip,
    /// C7: how long the epoch waited, and whether it is still waiting.
    Timing,
}

impl Check {
    pub const ALL: [Check; 7] = [
        Check::Inflow,
        Check::SwapOutput,
        Check::Settlement,
        Check::Landed,
        Check::FeeRate,
        Check::Tip,
        Check::Timing,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Check::Inflow => "C1",
            Check::SwapOutput => "C2",
            Check::Settlement => "C3",
            Check::Landed => "C4",
            Check::FeeRate => "C5",
            Check::Tip => "C6",
            Check::Timing => "C7",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Check::Inflow => "epoch inflow",
            Check::SwapOutput => "swap output",
            Check::Settlement => "settlement total",
            Check::Landed => "payouts landed",
            Check::FeeRate => "fee rate",
            Check::Tip => "tips",
            Check::Timing => "timing",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Outcome {
    Pass,
    Fail,
    /// Nothing to check yet: the epoch is open, or closed and not yet paid.
    Pending,
    /// Recorded for information, not pass or fail.
    Info,
    /// The check does not apply to this epoch.
    NotApplicable,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Pending => "pending",
            Outcome::Info => "info",
            Outcome::NotApplicable => "n/a",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub check: Check,
    pub outcome: Outcome,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub detail: String,
}

impl CheckResult {
    fn new(check: Check, outcome: Outcome, detail: impl Into<String>) -> Self {
        Self {
            check,
            outcome,
            expected: None,
            actual: None,
            detail: detail.into(),
        }
    }

    fn numbers(mut self, expected: impl ToString, actual: impl ToString) -> Self {
        self.expected = Some(expected.to_string());
        self.actual = Some(actual.to_string());
        self
    }
}

/// Runs C1 to C7 on one epoch. `now` (unix seconds) is used only for the ages in C7.
pub fn reconcile(e: &EpochData, now: i64) -> Vec<CheckResult> {
    // An epoch is closed, swapped and paid at most once. Anything else fails every check
    // that depends on it, with the counts in the detail.
    let duplicate = [
        ("closes", e.closes.len()),
        ("swaps", e.swaps.len()),
        ("payouts", e.payouts.len()),
    ]
    .into_iter()
    .find(|(_, n)| *n > 1);
    if let Some((what, n)) = duplicate {
        let detail = format!("{n} {what} recorded for one epoch");
        return Check::ALL
            .iter()
            .map(|&c| CheckResult::new(c, Outcome::Fail, detail.clone()))
            .collect();
    }
    let close = e.closes.first();
    let swap = e.swaps.first();
    let payout = e.payouts.first();
    vec![
        inflow(e, close, now),
        swap_output(e, swap, payout),
        settlement(e, close, swap, payout),
        landed(payout),
        fee_rate(e, payout),
        tip(e, payout),
        timing(e, close, payout, now),
    ]
}

fn sum_gross(e: &EpochData) -> u128 {
    e.donations.iter().map(|d| u128::from(d.gross)).sum()
}

fn oldest_donation(e: &EpochData) -> Option<i64> {
    e.donations.iter().filter_map(|d| d.block_time).min()
}

fn days(seconds: i64) -> String {
    format!("{:.1} days", seconds as f64 / DAY as f64)
}

/// C1: the sum of every donation and bridge admission in the epoch equals the closed total.
fn inflow(e: &EpochData, close: Option<&CloseRow>, now: i64) -> CheckResult {
    let donated = sum_gross(e);
    let Some(close) = close else {
        let age = oldest_donation(e).map_or("unknown age".into(), |t| days(now - t));
        return CheckResult::new(
            Check::Inflow,
            Outcome::Pending,
            format!(
                "open: {} donations, {donated} base units, oldest {age} old",
                e.donations.len()
            ),
        );
    };
    let outcome = if donated == u128::from(close.gross_epoch) {
        Outcome::Pass
    } else {
        Outcome::Fail
    };
    CheckResult::new(
        Check::Inflow,
        outcome,
        format!("{} donations", e.donations.len()),
    )
    .numbers(donated, close.gross_epoch)
}

/// C2: the swap's reported output equals what its output token account gained.
fn swap_output(e: &EpochData, swap: Option<&SwapRow>, payout: Option<&PayoutRow>) -> CheckResult {
    let Some(swap) = swap else {
        let detail = match payout {
            Some(p) if p.settlement_mint == e.mint => "no swap: already in the settlement mint",
            Some(p) if p.instruction == "swap_then_distribute_v1" => {
                "swapped and paid in one instruction, with no separate swap record"
            }
            Some(_) => "paid without a swap record",
            None => "not swapped",
        };
        return CheckResult::new(Check::SwapOutput, Outcome::NotApplicable, detail);
    };
    let received = swap
        .output_received
        .map_or("nothing".to_string(), |r| r.to_string());
    let outcome = if swap.output_received == Some(i128::from(swap.output)) {
        Outcome::Pass
    } else {
        Outcome::Fail
    };
    CheckResult::new(
        Check::SwapOutput,
        outcome,
        "reported output vs amount received",
    )
    .numbers(swap.output, received)
}

/// C3: payout lines add up to the settled amount, which equals the swap output (swapped
/// epochs) or the closed total (epochs already in the settlement mint). For older bps
/// payouts, the shares add up to 10,000 and every line is its share rounded down, plus at
/// most one unit to spread the remainder.
fn settlement(
    e: &EpochData,
    close: Option<&CloseRow>,
    swap: Option<&SwapRow>,
    payout: Option<&PayoutRow>,
) -> CheckResult {
    let Some(p) = payout else {
        let outcome = if close.is_some() {
            Outcome::Pending
        } else {
            Outcome::NotApplicable
        };
        return CheckResult::new(Check::Settlement, outcome, "not paid out");
    };
    let fail = |detail: &str, expected: u128, actual: u128| {
        CheckResult::new(Check::Settlement, Outcome::Fail, detail).numbers(expected, actual)
    };
    let lines: u128 = p.lines.iter().map(|l| u128::from(l.amount)).sum();
    if lines != u128::from(p.settled) {
        return fail(
            "payout lines do not add up to the settled amount",
            u128::from(p.settled),
            lines,
        );
    }
    let source = if let Some(swap) = swap {
        if swap.output != p.settled {
            return fail(
                "settled amount differs from the swap output",
                u128::from(swap.output),
                u128::from(p.settled),
            );
        }
        "swap output"
    } else if p.settlement_mint == e.mint {
        let Some(close) = close else {
            return CheckResult::new(
                Check::Settlement,
                Outcome::Fail,
                "paid out but never closed",
            );
        };
        if close.gross_epoch != p.settled {
            return fail(
                "settled amount differs from the closed total",
                u128::from(close.gross_epoch),
                u128::from(p.settled),
            );
        }
        "closed total"
    } else if p.instruction == "swap_then_distribute_v1" {
        "lines only (swap and payout in one instruction)"
    } else {
        return CheckResult::new(
            Check::Settlement,
            Outcome::Fail,
            "paid after a swap that was never recorded",
        );
    };
    if !p.exact {
        let bps: u64 = p.lines.iter().map(|l| u64::from(l.bps.unwrap_or(0))).sum();
        if bps != 10_000 {
            return fail(
                "shares do not add up to 10,000 bps",
                10_000,
                u128::from(bps),
            );
        }
        for line in &p.lines {
            let floor = u128::from(p.settled) * u128::from(line.bps.unwrap_or(0)) / 10_000;
            let amount = u128::from(line.amount);
            if amount < floor || amount > floor + 1 {
                return fail(
                    "a line is not its share rounded down, plus at most one unit",
                    floor,
                    amount,
                );
            }
        }
    }
    CheckResult::new(
        Check::Settlement,
        Outcome::Pass,
        format!("lines = settled = {source}"),
    )
    .numbers(p.settled, lines)
}

/// C4: for every recipient, the settlement mint held by token accounts it owns grew by
/// exactly the sum of its payout lines, in the payout transaction.
fn landed(payout: Option<&PayoutRow>) -> CheckResult {
    let Some(p) = payout else {
        return CheckResult::new(Check::Landed, Outcome::NotApplicable, "not paid out");
    };
    let mut owed: BTreeMap<&str, i128> = BTreeMap::new();
    for line in &p.lines {
        *owed.entry(line.address.as_str()).or_default() += i128::from(line.amount);
    }
    for (address, amount) in &owed {
        let received = p.received.get(*address).copied().unwrap_or(0);
        if received != *amount {
            return CheckResult::new(
                Check::Landed,
                Outcome::Fail,
                format!("{address} received a different amount"),
            )
            .numbers(amount, received);
        }
    }
    CheckResult::new(
        Check::Landed,
        Outcome::Pass,
        format!("{} recipients", owed.len()),
    )
}

/// C5: the fee is one known rate applied to the whole epoch. Exact payouts allow one unit of
/// rounding per donation; bps payouts state their rate, and C3 already checked its rounding.
///
/// The operator prices each donation by its own slot and attribution, so an epoch can mix rates
/// when a rate changes mid-epoch. Such an epoch passes only if its donations can be split
/// between two known rates in a way that reproduces the fee within the same rounding; the
/// split is reported. Anything else fails.
fn fee_rate(e: &EpochData, payout: Option<&PayoutRow>) -> CheckResult {
    let Some(p) = payout else {
        return CheckResult::new(Check::FeeRate, Outcome::NotApplicable, "not paid out");
    };
    let fee_lines: Vec<&LineRow> = p
        .lines
        .iter()
        .filter(|l| l.share_type == ShareType::Fee)
        .collect();
    let fee: u128 = fee_lines.iter().map(|l| u128::from(l.amount)).sum();
    if p.settled == 0 {
        return CheckResult::new(Check::FeeRate, Outcome::Info, "nothing settled");
    }
    let settled = u128::from(p.settled);
    let donations = e.donations.len().max(1) as u128;
    let (min, max) = (
        KNOWN_FEE_RATES_BPS[0],
        KNOWN_FEE_RATES_BPS[KNOWN_FEE_RATES_BPS.len() - 1],
    );

    let rate_bps: f64 = if p.exact {
        let tolerance = donations * 10_000;
        if let Some(rate) = KNOWN_FEE_RATES_BPS
            .iter()
            .find(|&&r| (fee * 10_000).abs_diff(settled * u128::from(r)) <= tolerance)
        {
            return CheckResult::new(Check::FeeRate, Outcome::Pass, format!("{rate} bps"))
                .numbers(format!("{rate} bps"), fee);
        }
        fee as f64 * 10_000.0 / settled as f64
    } else {
        let stated = fee_lines.first().and_then(|l| l.bps).map_or(0, u64::from);
        if KNOWN_FEE_RATES_BPS.contains(&stated) {
            return CheckResult::new(Check::FeeRate, Outcome::Pass, format!("{stated} bps"))
                .numbers(format!("{stated} bps"), fee);
        }
        stated as f64
    };

    let gross: Vec<u64> = e.donations.iter().map(|d| d.gross).collect();
    let total: u128 = gross.iter().map(|&g| u128::from(g)).sum();
    // The weighted rate sum W = sum(gross_i * rate_i). For exact payouts the fee is W/total of
    // the settled amount, give or take one unit per donation. For bps payouts the stated bps
    // is W/total, give or take one bps.
    let fits = |weighted: u128| -> bool {
        if p.exact {
            (fee * 10_000 * total).abs_diff(settled * weighted) <= donations * 10_000 * total
        } else {
            (rate_bps as u128 * total).abs_diff(weighted) <= total
        }
    };
    let detail = format!("{rate_bps:.0} bps on {} donations", e.donations.len());
    let failed = |reason: &str| {
        CheckResult::new(Check::FeeRate, Outcome::Fail, format!("{reason}: {detail}")).numbers(
            format!("one of {KNOWN_FEE_RATES_BPS:?} bps, or a split of donations between two"),
            format!("{rate_bps:.1} bps"),
        )
    };
    if e.donations.len() < 2 || total == 0 || rate_bps <= min as f64 || rate_bps >= max as f64 {
        return failed("not a known rate");
    }
    match explain_blend(&gross, fits) {
        Blend::Found {
            low,
            low_count,
            high,
            high_count,
        } => CheckResult::new(
            Check::FeeRate,
            Outcome::Pass,
            format!(
                "blended: {} at {low} bps and {high_count} at {high} bps",
                donations_text(low_count)
            ),
        )
        .numbers(
            "a split between two known rates",
            format!("{rate_bps:.1} bps"),
        ),
        Blend::TooMany => CheckResult::new(
            Check::FeeRate,
            Outcome::Info,
            format!("blended, too many donations to test every split: {detail}"),
        ),
        Blend::None => failed("no split between two known rates gives this fee"),
    }
}

fn donations_text(n: usize) -> String {
    if n == 1 {
        "1 donation".into()
    } else {
        format!("{n} donations")
    }
}

/// The most donations [`explain_blend`] tries every split of: 2^20 splits per pair of rates.
const MAX_BLEND_DONATIONS: usize = 20;

#[derive(Debug, PartialEq, Eq)]
enum Blend {
    Found {
        low: u64,
        low_count: usize,
        high: u64,
        high_count: usize,
    },
    TooMany,
    None,
}

/// Looks for a pair of known rates and a split of the donations between them whose weighted
/// rate sum satisfies `fits`. Each donation is charged one rate, and both rates are used.
fn explain_blend(gross: &[u64], fits: impl Fn(u128) -> bool) -> Blend {
    let n = gross.len();
    if n > MAX_BLEND_DONATIONS {
        return Blend::TooMany;
    }
    let total: u128 = gross.iter().map(|&g| u128::from(g)).sum();
    // subset[mask] = total gross of the donations in `mask`.
    let mut subset = vec![0u128; 1 << n];
    for mask in 1..subset.len() {
        let lowest = mask.trailing_zeros() as usize;
        subset[mask] = subset[mask & (mask - 1)] + u128::from(gross[lowest]);
    }
    let full = subset.len() - 1;
    for (i, &low) in KNOWN_FEE_RATES_BPS.iter().enumerate() {
        for &high in &KNOWN_FEE_RATES_BPS[i + 1..] {
            for (mask, &at_high) in subset.iter().enumerate().take(full).skip(1) {
                let weighted = u128::from(low) * total + u128::from(high - low) * at_high;
                if fits(weighted) {
                    let high_count = mask.count_ones() as usize;
                    return Blend::Found {
                        low,
                        low_count: n - high_count,
                        high,
                        high_count,
                    };
                }
            }
        }
    }
    Blend::None
}

/// C6: tips declared by donors were paid as tip lines, in proportion to the settled amount,
/// within one unit of rounding per donation.
fn tip(e: &EpochData, payout: Option<&PayoutRow>) -> CheckResult {
    let Some(p) = payout else {
        return CheckResult::new(Check::Tip, Outcome::NotApplicable, "not paid out");
    };
    let declared: u128 = e.donations.iter().map(|d| u128::from(d.tip)).sum();
    let paid: u128 = p
        .lines
        .iter()
        .filter(|l| l.share_type == ShareType::Tip)
        .map(|l| u128::from(l.amount))
        .sum();
    if declared == 0 {
        let outcome = if paid == 0 {
            Outcome::Pass
        } else {
            Outcome::Fail
        };
        return CheckResult::new(Check::Tip, outcome, "no tips declared").numbers(0, paid);
    }
    let gross = sum_gross(e);
    if gross == 0 {
        return CheckResult::new(Check::Tip, Outcome::Fail, "tips declared on zero donations")
            .numbers(0, paid);
    }
    let expected = u128::from(p.settled) * declared / gross;
    let tolerance = e.donations.len() as u128;
    let outcome = if paid.abs_diff(expected) <= tolerance {
        Outcome::Pass
    } else {
        Outcome::Fail
    };
    CheckResult::new(Check::Tip, outcome, "tips in proportion to settled").numbers(expected, paid)
}

/// C7: time from the oldest donation to the payout, or how long the epoch has been waiting.
fn timing(
    e: &EpochData,
    close: Option<&CloseRow>,
    payout: Option<&PayoutRow>,
    now: i64,
) -> CheckResult {
    let oldest = oldest_donation(e);
    match (close, payout) {
        (_, Some(p)) => {
            let waited = match (oldest, p.block_time) {
                (Some(start), Some(end)) => end - start,
                _ => 0,
            };
            CheckResult::new(
                Check::Timing,
                Outcome::Info,
                format!("paid {} after the oldest donation", days(waited)),
            )
            .numbers("", waited)
        }
        (Some(c), None) => {
            let since = c.block_time.map_or(0, |t| now - t);
            CheckResult::new(
                Check::Timing,
                Outcome::Pending,
                format!("closed {} ago, not paid out", days(since)),
            )
            .numbers("", since)
        }
        (None, None) => {
            let since = oldest.map_or(0, |t| now - t);
            CheckResult::new(
                Check::Timing,
                Outcome::Pending,
                format!("open, oldest donation {} old", days(since)),
            )
            .numbers("", since)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOL: &str = "So11111111111111111111111111111111111111112";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const NOW: i64 = 1_800_000_000;

    fn donation(gross: u64) -> DonationRow {
        DonationRow {
            signature: "d".into(),
            gross,
            tip: 0,
            block_time: Some(NOW - 3_600),
        }
    }

    fn line(share_type: ShareType, address: &str, amount: u64, bps: Option<u16>) -> LineRow {
        LineRow {
            share_type,
            address: address.into(),
            amount,
            bps,
        }
    }

    /// The real SOL epoch from the `payout-exact-after-swap` fixture: 160.251464 USDC settled,
    /// 128.201171 to the charity and 32.050293 (20%) as fee.
    fn swapped_epoch() -> EpochData {
        let settled = 160_251_464;
        EpochData {
            config_id: "0x4f01".into(),
            mint: SOL.into(),
            epoch: 2,
            donations: vec![donation(1_000_000_000)],
            closes: vec![CloseRow {
                signature: "c".into(),
                gross_epoch: 1_000_000_000,
                block_time: Some(NOW - 1_800),
            }],
            swaps: vec![SwapRow {
                signature: "s".into(),
                output: settled,
                output_received: Some(i128::from(settled)),
                block_time: Some(NOW - 900),
            }],
            payouts: vec![PayoutRow {
                signature: "p".into(),
                instruction: "distribute_exact_after_just_swapping_v1".into(),
                exact: true,
                settled,
                settlement_mint: USDC.into(),
                lines: vec![
                    line(ShareType::Beneficiary, "charity", 128_201_171, None),
                    line(ShareType::Fee, "fee", 32_050_293, None),
                ],
                received: BTreeMap::from([
                    ("charity".into(), 128_201_171),
                    ("fee".into(), 32_050_293),
                ]),
                block_time: Some(NOW),
            }],
        }
    }

    fn outcome(e: &EpochData, check: Check) -> CheckResult {
        reconcile(e, NOW)
            .into_iter()
            .find(|r| r.check == check)
            .unwrap()
    }

    #[test]
    fn a_clean_swapped_epoch_passes_everything() {
        let results = reconcile(&swapped_epoch(), NOW);
        let outcomes: Vec<(Check, Outcome)> =
            results.iter().map(|r| (r.check, r.outcome)).collect();
        assert_eq!(
            outcomes,
            [
                (Check::Inflow, Outcome::Pass),
                (Check::SwapOutput, Outcome::Pass),
                (Check::Settlement, Outcome::Pass),
                (Check::Landed, Outcome::Pass),
                (Check::FeeRate, Outcome::Pass),
                (Check::Tip, Outcome::Pass),
                (Check::Timing, Outcome::Info),
            ]
        );
        assert_eq!(outcome(&swapped_epoch(), Check::FeeRate).detail, "2000 bps");
    }

    #[test]
    fn missing_donation_fails_inflow() {
        let mut e = swapped_epoch();
        e.donations[0].gross -= 1;
        let r = outcome(&e, Check::Inflow);
        assert_eq!(r.outcome, Outcome::Fail);
        assert_eq!(
            (r.expected.as_deref(), r.actual.as_deref()),
            (Some("999999999"), Some("1000000000"))
        );
    }

    #[test]
    fn swap_output_that_never_arrived_fails() {
        let mut e = swapped_epoch();
        e.swaps[0].output_received = Some(160_000_000);
        assert_eq!(outcome(&e, Check::SwapOutput).outcome, Outcome::Fail);
    }

    #[test]
    fn lines_that_do_not_add_up_fail_settlement() {
        let mut e = swapped_epoch();
        e.payouts[0].lines[0].amount -= 1;
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Fail);
    }

    #[test]
    fn settled_amount_must_match_the_swap() {
        let mut e = swapped_epoch();
        e.swaps[0].output += 1;
        e.swaps[0].output_received = Some(i128::from(e.swaps[0].output));
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Fail);
    }

    #[test]
    fn a_payout_that_did_not_arrive_fails_landed() {
        let mut e = swapped_epoch();
        e.payouts[0].received.insert("charity".into(), 128_201_170);
        let r = outcome(&e, Check::Landed);
        assert_eq!(r.outcome, Outcome::Fail);
        assert!(r.detail.starts_with("charity"));
    }

    #[test]
    fn bps_lines_must_be_their_share_rounded() {
        let mut e = swapped_epoch();
        let p = &mut e.payouts[0];
        p.exact = false;
        p.instruction = "distribute_after_just_swapping_v1".into();
        // 10% of 160,251,464 is 16,025,146.4, so the fee line is 16,025,146 or 16,025,147.
        p.lines = vec![
            line(ShareType::Beneficiary, "charity", 144_226_317, Some(9_000)),
            line(ShareType::Fee, "fee", 16_025_147, Some(1_000)),
        ];
        p.received = BTreeMap::from([("charity".into(), 144_226_317), ("fee".into(), 16_025_147)]);
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Pass);
        assert_eq!(outcome(&e, Check::FeeRate).detail, "1000 bps");

        e.payouts[0].lines[1].amount += 1;
        e.payouts[0].lines[0].amount -= 1;
        assert_eq!(
            outcome(&e, Check::Settlement).outcome,
            Outcome::Fail,
            "two units over its share"
        );
    }

    #[test]
    fn already_in_usdc_settles_the_closed_total() {
        let mut e = swapped_epoch();
        e.mint = USDC.into();
        e.swaps.clear();
        e.payouts[0].instruction = "distribute_exact_non_swap_output_mint_v1".into();
        e.donations = vec![donation(160_251_464)];
        e.closes[0].gross_epoch = 160_251_464;
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Pass);
        assert_eq!(
            outcome(&e, Check::SwapOutput).outcome,
            Outcome::NotApplicable
        );
        e.closes[0].gross_epoch += 5;
        e.donations[0].gross += 5;
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Fail);
    }

    #[test]
    fn mixed_rates_pass_when_a_split_explains_them() {
        let mut e = swapped_epoch();
        // 19.87%: one donation of 13M at 10% and one of 987M at 20% gives exactly that.
        e.donations = vec![donation(13_000_000), donation(987_000_000)];
        let p = &mut e.payouts[0];
        p.exact = false;
        p.lines = vec![
            line(ShareType::Beneficiary, "charity", 128_409_498, Some(8_013)),
            line(ShareType::Fee, "fee", 31_841_966, Some(1_987)),
        ];
        p.received = BTreeMap::from([("charity".into(), 128_409_498), ("fee".into(), 31_841_966)]);
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Pass);
        let r = outcome(&e, Check::FeeRate);
        assert_eq!(r.outcome, Outcome::Pass);
        assert_eq!(
            r.detail,
            "blended: 1 donation at 1000 bps and 1 at 2000 bps"
        );

        // Two equal donations cannot make 19.87% from any two known rates.
        e.donations = vec![donation(500_000_000), donation(500_000_000)];
        let r = outcome(&e, Check::FeeRate);
        assert_eq!(r.outcome, Outcome::Fail);
        assert!(r.detail.starts_with("no split"));

        // One donation can never be blended.
        e.donations = vec![donation(1_000_000_000)];
        assert_eq!(outcome(&e, Check::FeeRate).outcome, Outcome::Fail);
    }

    #[test]
    fn exact_blend_between_3_and_20_percent() {
        let mut e = swapped_epoch();
        // 2,000 at 3% plus 8,000 at 20% is 16.6% of 10,000.
        e.donations = vec![donation(2_000), donation(8_000)];
        let p = &mut e.payouts[0];
        p.settled = 10_000;
        e.swaps[0].output = 10_000;
        e.swaps[0].output_received = Some(10_000);
        p.lines = vec![
            line(ShareType::Beneficiary, "charity", 8_340, None),
            line(ShareType::Fee, "fee", 1_660, None),
        ];
        p.received = BTreeMap::from([("charity".into(), 8_340), ("fee".into(), 1_660)]);
        let r = outcome(&e, Check::FeeRate);
        assert_eq!(r.outcome, Outcome::Pass, "{}", r.detail);
        assert!(r.detail.contains("300 bps") && r.detail.contains("2000 bps"));
    }

    #[test]
    fn too_many_donations_to_test_every_split_is_info() {
        let fits = |_: u128| false;
        assert_eq!(
            explain_blend(&[1; MAX_BLEND_DONATIONS + 1], fits),
            Blend::TooMany
        );
        assert_eq!(explain_blend(&[1, 2, 3], fits), Blend::None);
    }

    #[test]
    fn exact_fee_allows_one_unit_of_rounding_per_donation() {
        let mut e = swapped_epoch();
        // 20% of 160,251,464 is 32,050,292.8: 32,050,292 and 32,050,293 both pass.
        e.payouts[0].lines[1].amount = 32_050_292;
        e.payouts[0].lines[0].amount = 128_201_172;
        e.payouts[0].received =
            BTreeMap::from([("charity".into(), 128_201_172), ("fee".into(), 32_050_292)]);
        assert_eq!(outcome(&e, Check::FeeRate).outcome, Outcome::Pass);
        // 21% is not a known rate.
        e.payouts[0].lines[1].amount = 33_652_807;
        e.payouts[0].lines[0].amount = 160_251_464 - 33_652_807;
        assert_eq!(outcome(&e, Check::FeeRate).outcome, Outcome::Fail);
    }

    #[test]
    fn tips_are_paid_in_proportion() {
        let mut e = swapped_epoch();
        // A real case: 1 bps tipped on 0.012 SOL, paid as 104 of 1,043,548 settled.
        e.donations = vec![DonationRow {
            signature: "d".into(),
            gross: 12_000_000,
            tip: 1_200,
            block_time: Some(NOW),
        }];
        e.closes[0].gross_epoch = 12_000_000;
        let p = &mut e.payouts[0];
        p.settled = 1_043_548;
        e.swaps[0].output = 1_043_548;
        e.swaps[0].output_received = Some(1_043_548);
        p.lines = vec![
            line(ShareType::Beneficiary, "charity", 1_012_242, None),
            line(ShareType::Fee, "fee", 31_202, None),
            line(ShareType::Tip, "fee", 104, None),
        ];
        p.received = BTreeMap::from([("charity".into(), 1_012_242), ("fee".into(), 31_306)]);
        assert_eq!(outcome(&e, Check::Tip).outcome, Outcome::Pass);
        assert_eq!(
            outcome(&e, Check::Landed).outcome,
            Outcome::Pass,
            "fee and tip to one address are summed"
        );
    }

    #[test]
    fn open_and_unpaid_epochs_are_pending() {
        let mut e = swapped_epoch();
        e.payouts.clear();
        e.swaps.clear();
        assert_eq!(outcome(&e, Check::Timing).outcome, Outcome::Pending);
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Pending);
        assert_eq!(outcome(&e, Check::Inflow).outcome, Outcome::Pass);
        e.closes.clear();
        let r = outcome(&e, Check::Inflow);
        assert_eq!(r.outcome, Outcome::Pending);
        assert!(r.detail.starts_with("open"));
    }

    #[test]
    fn a_payout_after_a_missing_swap_fails() {
        let mut e = swapped_epoch();
        e.swaps.clear();
        assert_eq!(outcome(&e, Check::Settlement).outcome, Outcome::Fail);
        e.payouts[0].instruction = "swap_then_distribute_v1".into();
        assert_eq!(
            outcome(&e, Check::Settlement).outcome,
            Outcome::Pass,
            "the older one-step swap and payout"
        );
    }

    #[test]
    fn two_payouts_for_one_epoch_fail_everything() {
        let mut e = swapped_epoch();
        e.payouts.push(e.payouts[0].clone());
        assert!(
            reconcile(&e, NOW)
                .iter()
                .all(|r| r.outcome == Outcome::Fail)
        );
    }
}
