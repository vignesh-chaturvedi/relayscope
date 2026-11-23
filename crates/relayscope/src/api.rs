//! Read-only queries behind the HTTP API. Each function takes a SQLite connection and returns
//! a serializable value. Amounts are exact decimal strings of base units, because JavaScript
//! numbers lose precision above 2^53.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use relayscope_core::{Bytes32, Pubkey};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::known;

/// Token decimals, learned from the token balance changes in stored transactions.
#[derive(Clone, Debug, Default)]
pub struct Tokens {
    decimals: HashMap<String, u8>,
}

impl Tokens {
    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt =
            conn.prepare("SELECT mint, MAX(decimals) FROM token_changes GROUP BY mint")?;
        let decimals = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Self { decimals })
    }

    pub fn amount(&self, mint: &str, raw: u128) -> Amount {
        Amount {
            raw: raw.to_string(),
            mint: mint.to_string(),
            symbol: known::symbol(mint),
            decimals: self.decimals.get(mint).copied(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Amount {
    /// Base units, as a decimal string.
    pub raw: String,
    pub mint: String,
    pub symbol: Option<&'static str>,
    pub decimals: Option<u8>,
}

fn base_units(v: i64) -> u128 {
    u128::from(u64::try_from(v).unwrap_or_default())
}

#[derive(Clone, Debug, Serialize)]
pub struct Pool {
    pub config_id: String,
    pub mint: String,
    pub epoch: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DonationBrief {
    pub signature: String,
    pub ordinal: i64,
    /// `direct` or `bridge`.
    pub kind: String,
    pub pool: Pool,
    pub amount: Amount,
    pub tip: String,
    pub message: Option<String>,
    pub credited_to: Option<String>,
    pub donor: Option<String>,
    pub invoked_by: Option<String>,
    pub caller: Option<&'static str>,
    pub slot: i64,
    pub block_time: Option<i64>,
    /// `false` while only seen at `confirmed` commitment.
    pub finalized: bool,
}

const DONATION_COLUMNS: &str = "d.signature, d.ordinal, d.kind, d.config_id, d.mint, d.epoch, d.gross_amount, d.tip, \
     d.message, d.credited_to, d.donor, d.invoked_by, d.slot, d.block_time, COALESCE(s.status, 1)";

fn donation_from_row(tokens: &Tokens, r: &rusqlite::Row<'_>) -> rusqlite::Result<DonationBrief> {
    let mint: String = r.get(4)?;
    let invoked_by: Option<String> = r.get(11)?;
    Ok(DonationBrief {
        signature: r.get(0)?,
        ordinal: r.get(1)?,
        kind: r.get(2)?,
        amount: tokens.amount(&mint, base_units(r.get(6)?)),
        pool: Pool {
            config_id: r.get(3)?,
            mint,
            epoch: r.get(5)?,
        },
        tip: base_units(r.get(7)?).to_string(),
        message: r.get::<_, Option<String>>(8)?.filter(|m| !m.is_empty()),
        credited_to: r.get(9)?,
        donor: r.get(10)?,
        caller: invoked_by.as_deref().and_then(known::program_name),
        invoked_by,
        slot: r.get(12)?,
        block_time: r.get(13)?,
        finalized: r.get::<_, i64>(14)? == 1,
    })
}

fn donations(
    conn: &Connection,
    tokens: &Tokens,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<DonationBrief>> {
    let sql = format!(
        "SELECT {DONATION_COLUMNS} FROM donations d LEFT JOIN signatures s USING (signature) {filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(args, |r| donation_from_row(tokens, r))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Donations after `slot`, oldest first, for the live feed.
pub fn donations_after(
    conn: &Connection,
    tokens: &Tokens,
    slot: i64,
) -> Result<Vec<DonationBrief>> {
    donations(
        conn,
        tokens,
        "WHERE d.slot > ?1 ORDER BY d.slot, d.signature, d.ordinal LIMIT 500",
        &[&slot],
    )
}

/// Donations to one config at or after `slot`, oldest first, for a feed that reconnects.
pub fn config_donations_since(
    conn: &Connection,
    tokens: &Tokens,
    config_id: &str,
    slot: i64,
    limit: i64,
) -> Result<Vec<DonationBrief>> {
    donations(
        conn,
        tokens,
        "WHERE d.slot >= ?2 AND d.config_id = ?1 ORDER BY d.slot, d.signature, d.ordinal LIMIT ?3",
        &[&config_id, &slot, &limit],
    )
}

pub fn latest_slot(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COALESCE(MAX(slot), 0) FROM donations", [], |r| {
            r.get(0)
        })?,
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub signature: String,
    pub block_time: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Close {
    #[serde(flatten)]
    pub step: Step,
    pub gross: Amount,
}

#[derive(Clone, Debug, Serialize)]
pub struct Swap {
    #[serde(flatten)]
    pub step: Step,
    pub output: Amount,
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    /// `Beneficiary`, `Fee` or `Tip`.
    pub share_type: String,
    pub address: String,
    pub amount: Amount,
    pub bps: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Payout {
    #[serde(flatten)]
    pub step: Step,
    pub instruction: Option<String>,
    pub exact: bool,
    pub settled: Amount,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckRow {
    pub id: String,
    pub outcome: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Epoch {
    pub pool: Pool,
    /// `open`, `closed`, `swapped` or `paid`.
    pub status: &'static str,
    pub donation_count: usize,
    pub donated: Amount,
    /// Up to 500 donations, oldest first.
    pub donations: Vec<DonationBrief>,
    pub close: Option<Close>,
    pub swap: Option<Swap>,
    pub payout: Option<Payout>,
    pub checks: Vec<CheckRow>,
}

/// Everything recorded for one `(config_id, mint, epoch)`, or `None` if nothing is.
pub fn epoch(
    conn: &Connection,
    tokens: &Tokens,
    config_id: &str,
    mint: &str,
    epoch: i64,
) -> Result<Option<Epoch>> {
    let mut all = donations(
        conn,
        tokens,
        "WHERE d.config_id = ?1 AND d.mint = ?2 AND d.epoch = ?3 ORDER BY d.slot, d.signature, d.ordinal",
        &[&config_id, &mint, &epoch],
    )?;
    let donated: u128 = all
        .iter()
        .map(|d| d.amount.raw.parse::<u128>().unwrap_or(0))
        .sum();
    let donation_count = all.len();
    all.truncate(500);

    let close = conn
        .query_row(
            "SELECT signature, block_time, gross_epoch_amount FROM epoch_closes WHERE config_id = ?1 AND mint = ?2 AND epoch = ?3",
            params![config_id, mint, epoch],
            |r| Ok(Close { step: Step { signature: r.get(0)?, block_time: r.get(1)? }, gross: tokens.amount(mint, base_units(r.get(2)?)) }),
        )
        .optional()?;
    let swap = conn
        .query_row(
            "SELECT signature, block_time, swap_output_amount, settlement_mint FROM epoch_swaps WHERE config_id = ?1 AND mint = ?2 AND epoch = ?3",
            params![config_id, mint, epoch],
            |r| {
                let settlement: String = r.get(3)?;
                Ok(Swap { step: Step { signature: r.get(0)?, block_time: r.get(1)? }, output: tokens.amount(&settlement, base_units(r.get(2)?)) })
            },
        )
        .optional()?;
    let payout_row = conn
        .query_row(
            "SELECT signature, ordinal, block_time, instruction, generation, settled_amount, settlement_mint
             FROM epoch_payouts WHERE config_id = ?1 AND mint = ?2 AND epoch = ?3",
            params![config_id, mint, epoch],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let payout = match payout_row {
        None => None,
        Some((signature, ordinal, block_time, instruction, generation, settled, settlement)) => {
            let mut stmt = conn.prepare(
                "SELECT share_type, address, amount, bps FROM payout_lines WHERE signature = ?1 AND ordinal = ?2 ORDER BY line",
            )?;
            let lines = stmt
                .query_map(params![signature, ordinal], |r| {
                    Ok(Line {
                        share_type: r.get(0)?,
                        address: r.get(1)?,
                        amount: tokens.amount(&settlement, base_units(r.get(2)?)),
                        bps: r.get(3)?,
                    })
                })?
                .collect::<Result<_, _>>()?;
            Some(Payout {
                step: Step {
                    signature,
                    block_time,
                },
                instruction,
                exact: generation == "exact",
                settled: tokens.amount(&settlement, base_units(settled)),
                lines,
            })
        }
    };
    if all.is_empty() && close.is_none() && swap.is_none() && payout.is_none() {
        return Ok(None);
    }
    let mut stmt = conn.prepare(
        "SELECT check_id, outcome, expected, actual, detail FROM checks
         WHERE config_id = ?1 AND mint = ?2 AND epoch = ?3 ORDER BY check_id",
    )?;
    let checks = stmt
        .query_map(params![config_id, mint, epoch], |r| {
            Ok(CheckRow {
                id: r.get(0)?,
                outcome: r.get(1)?,
                expected: r.get(2)?,
                actual: r.get(3)?,
                detail: r.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let status = match (&close, &swap, &payout) {
        (_, _, Some(_)) => "paid",
        (_, Some(_), None) => "swapped",
        (Some(_), None, None) => "closed",
        _ => "open",
    };
    Ok(Some(Epoch {
        pool: Pool {
            config_id: config_id.into(),
            mint: mint.into(),
            epoch,
        },
        status,
        donation_count,
        donated: tokens.amount(mint, donated),
        donations: all,
        close,
        swap,
        payout,
        checks,
    }))
}

/// One donation's estimated part of its epoch's payout. The program pays per epoch, so each
/// line is the epoch's line times `donation / epoch total`, rounded down.
#[derive(Clone, Debug, Serialize)]
pub struct Share {
    pub of_epoch: String,
    pub lines: Vec<Line>,
}

pub fn share(donation: &DonationBrief, epoch: &Epoch) -> Option<Share> {
    let payout = epoch.payout.as_ref()?;
    let part: u128 = donation.amount.raw.parse().ok()?;
    let total: u128 = epoch.close.as_ref()?.gross.raw.parse().ok()?;
    if total == 0 {
        return None;
    }
    let lines = payout
        .lines
        .iter()
        .map(|l| {
            let amount: u128 = l.amount.raw.parse().unwrap_or(0);
            Line {
                amount: Amount {
                    raw: (amount * part / total).to_string(),
                    ..l.amount.clone()
                },
                ..l.clone()
            }
        })
        .collect();
    Some(Share {
        of_epoch: format!("{part}/{total}"),
        lines,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    pub name: String,
    pub invoked_by: Option<String>,
    pub caller: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Touched {
    /// `closed`, `swapped` or `paid`.
    pub action: &'static str,
    pub pool: Pool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Transaction {
    pub signature: String,
    pub slot: i64,
    pub block_time: Option<i64>,
    pub succeeded: bool,
    pub relay_error: Option<String>,
    pub fee_payer: String,
    pub finalized: bool,
    pub instructions: Vec<Instruction>,
    pub donations: Vec<DonationBrief>,
    pub touched: Vec<Touched>,
}

pub fn transaction(
    conn: &Connection,
    tokens: &Tokens,
    signature: &str,
) -> Result<Option<Transaction>> {
    let row = conn
        .query_row(
            "SELECT t.slot, t.block_time, t.succeeded, t.relay_error, t.fee_payer, COALESCE(s.status, 1)
             FROM transactions t LEFT JOIN signatures s USING (signature) WHERE t.signature = ?1",
            [signature],
            |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)?)),
        )
        .optional()?;
    let Some((slot, block_time, succeeded, relay_error, fee_payer, status)) = row else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT name, invoked_by FROM instructions WHERE signature = ?1 ORDER BY ordinal",
    )?;
    let instructions = stmt
        .query_map([signature], |r| {
            let invoked_by: Option<String> = r.get(1)?;
            Ok(Instruction {
                name: r.get(0)?,
                caller: invoked_by.as_deref().and_then(known::program_name),
                invoked_by,
            })
        })?
        .collect::<Result<_, _>>()?;
    let donations = donations(
        conn,
        tokens,
        "WHERE d.signature = ?1 ORDER BY d.ordinal",
        &[&signature],
    )?;
    let mut touched = Vec::new();
    for (table, action) in [
        ("epoch_closes", "closed"),
        ("epoch_swaps", "swapped"),
        ("epoch_payouts", "paid"),
    ] {
        let mut stmt = conn.prepare(&format!(
            "SELECT config_id, mint, epoch FROM {table} WHERE signature = ?1"
        ))?;
        let rows = stmt.query_map([signature], |r| {
            Ok(Pool {
                config_id: r.get(0)?,
                mint: r.get(1)?,
                epoch: r.get(2)?,
            })
        })?;
        for pool in rows {
            touched.push(Touched {
                action,
                pool: pool?,
            });
        }
    }
    Ok(Some(Transaction {
        signature: signature.into(),
        slot,
        block_time,
        succeeded,
        relay_error,
        fee_payer,
        finalized: status == 1,
        instructions,
        donations,
        touched,
    }))
}

#[derive(Clone, Debug, Serialize)]
pub struct EpochSummary {
    pub mint: String,
    pub epoch: i64,
    pub status: &'static str,
    pub donation_count: i64,
    pub donated: Amount,
    pub last_activity: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Config {
    pub config_id: String,
    pub base58: String,
    pub donation_count: i64,
    pub donated: Vec<Amount>,
    pub paid_to_beneficiaries: Vec<Amount>,
    pub epoch_count: usize,
    /// The 500 most recently active epochs.
    pub epochs: Vec<EpochSummary>,
}

pub fn config(conn: &Connection, tokens: &Tokens, config_id: &str) -> Result<Option<Config>> {
    let mut epochs: BTreeMap<(String, i64), EpochSummary> = BTreeMap::new();
    let mut donated: BTreeMap<String, u128> = BTreeMap::new();
    let mut donation_count = 0;
    {
        let mut stmt = conn.prepare(
            "SELECT mint, epoch, COUNT(*), MAX(block_time), GROUP_CONCAT(gross_amount) FROM donations
             WHERE config_id = ?1 GROUP BY mint, epoch",
        )?;
        let mut rows = stmt.query([config_id])?;
        while let Some(r) = rows.next()? {
            let mint: String = r.get(0)?;
            let epoch: i64 = r.get(1)?;
            let count: i64 = r.get(2)?;
            let sum: u128 = r
                .get::<_, String>(4)?
                .split(',')
                .filter_map(|v| v.parse::<u128>().ok())
                .sum();
            donation_count += count;
            *donated.entry(mint.clone()).or_default() += sum;
            epochs.insert(
                (mint.clone(), epoch),
                EpochSummary {
                    donated: tokens.amount(&mint, sum),
                    mint,
                    epoch,
                    status: "open",
                    donation_count: count,
                    last_activity: r.get(3)?,
                },
            );
        }
    }
    for (table, status) in [
        ("epoch_closes", "closed"),
        ("epoch_swaps", "swapped"),
        ("epoch_payouts", "paid"),
    ] {
        let mut stmt = conn.prepare(&format!(
            "SELECT mint, epoch, block_time FROM {table} WHERE config_id = ?1"
        ))?;
        let mut rows = stmt.query([config_id])?;
        while let Some(r) = rows.next()? {
            let mint: String = r.get(0)?;
            let epoch: i64 = r.get(1)?;
            let at: Option<i64> = r.get(2)?;
            let entry = epochs
                .entry((mint.clone(), epoch))
                .or_insert_with(|| EpochSummary {
                    donated: tokens.amount(&mint, 0),
                    mint,
                    epoch,
                    status,
                    donation_count: 0,
                    last_activity: at,
                });
            entry.status = status;
            entry.last_activity = entry.last_activity.max(at);
        }
    }
    if epochs.is_empty() {
        return Ok(None);
    }
    let mut paid: BTreeMap<String, u128> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT p.settlement_mint, l.amount FROM epoch_payouts p JOIN payout_lines l USING (signature, ordinal)
             WHERE p.config_id = ?1 AND l.share_type = 'Beneficiary'",
        )?;
        let mut rows = stmt.query([config_id])?;
        while let Some(r) = rows.next()? {
            *paid.entry(r.get(0)?).or_default() += base_units(r.get(1)?);
        }
    }
    let epoch_count = epochs.len();
    let mut list: Vec<EpochSummary> = epochs.into_values().collect();
    list.sort_by_key(|e| std::cmp::Reverse((e.last_activity, e.epoch)));
    list.truncate(500);
    let base58 = config_id
        .parse::<Bytes32>()
        .map(|b| b.to_base58())
        .unwrap_or_default();
    Ok(Some(Config {
        config_id: config_id.into(),
        base58,
        donation_count,
        donated: donated.iter().map(|(m, v)| tokens.amount(m, *v)).collect(),
        paid_to_beneficiaries: paid.iter().map(|(m, v)| tokens.amount(m, *v)).collect(),
        epoch_count,
        epochs: list,
    }))
}

#[derive(Clone, Debug, Serialize)]
pub struct Received {
    pub share_type: String,
    pub amount: Amount,
    pub signature: String,
    pub block_time: Option<i64>,
    pub pool: Pool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Address {
    pub address: String,
    pub donation_count: i64,
    /// The 200 newest donations made from or credited to this address.
    pub donations: Vec<DonationBrief>,
    pub received_count: i64,
    /// The 200 newest payout lines paid to this address.
    pub received: Vec<Received>,
}

pub fn address(conn: &Connection, tokens: &Tokens, address: &str) -> Result<Address> {
    let donation_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM donations WHERE donor = ?1 OR credited_to = ?1",
        [address],
        |r| r.get(0),
    )?;
    let donations = donations(
        conn,
        tokens,
        "WHERE d.donor = ?1 OR d.credited_to = ?1 ORDER BY d.slot DESC LIMIT 200",
        &[&address],
    )?;
    let received_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM payout_lines WHERE address = ?1",
        [address],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT l.share_type, l.amount, l.signature, p.block_time, p.config_id, p.mint, p.epoch, p.settlement_mint
         FROM payout_lines l JOIN epoch_payouts p USING (signature, ordinal)
         WHERE l.address = ?1 ORDER BY p.slot DESC LIMIT 200",
    )?;
    let received = stmt
        .query_map([address], |r| {
            let settlement: String = r.get(7)?;
            Ok(Received {
                share_type: r.get(0)?,
                amount: tokens.amount(&settlement, base_units(r.get(1)?)),
                signature: r.get(2)?,
                block_time: r.get(3)?,
                pool: Pool {
                    config_id: r.get(4)?,
                    mint: r.get(5)?,
                    epoch: r.get(6)?,
                },
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(Address {
        address: address.into(),
        donation_count,
        donations,
        received_count,
        received,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckSummary {
    pub id: String,
    pub pass: i64,
    pub fail: i64,
    pub pending: i64,
    pub info: i64,
    pub not_applicable: i64,
}

pub fn check_summary(conn: &Connection) -> Result<Vec<CheckSummary>> {
    let mut by: BTreeMap<String, CheckSummary> = BTreeMap::new();
    let mut stmt =
        conn.prepare("SELECT check_id, outcome, COUNT(*) FROM checks GROUP BY check_id, outcome")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id: String = r.get(0)?;
        let outcome: String = r.get(1)?;
        let n: i64 = r.get(2)?;
        let entry = by.entry(id.clone()).or_insert(CheckSummary {
            id,
            pass: 0,
            fail: 0,
            pending: 0,
            info: 0,
            not_applicable: 0,
        });
        match outcome.as_str() {
            "pass" => entry.pass = n,
            "fail" => entry.fail = n,
            "pending" => entry.pending = n,
            "info" => entry.info = n,
            _ => entry.not_applicable = n,
        }
    }
    Ok(by.into_values().collect())
}

#[derive(Clone, Debug, Serialize)]
pub struct Indexing {
    pub transactions: i64,
    pub first_block_time: Option<i64>,
    pub last_block_time: Option<i64>,
    pub awaiting_finalization: i64,
    pub queued: i64,
    pub decode_failures: i64,
    pub history_complete: bool,
}

pub fn indexing(conn: &Connection) -> Result<Indexing> {
    let one = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(sql, [], |r| r.get(0)) };
    let (first, last) = conn.query_row(
        "SELECT MIN(block_time), MAX(block_time) FROM transactions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Indexing {
        transactions: one("SELECT COUNT(*) FROM transactions")?,
        first_block_time: first,
        last_block_time: last,
        awaiting_finalization: one("SELECT COUNT(*) FROM signatures WHERE status = 3")?,
        queued: one("SELECT COUNT(*) FROM signatures WHERE status = 0")?,
        decode_failures: one("SELECT COUNT(*) FROM decode_failures")?,
        history_complete: conn
            .query_row(
                "SELECT 1 FROM state WHERE key = 'history_complete'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some(),
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct Overview {
    pub indexing: Indexing,
    pub donation_count: i64,
    pub donated: Vec<Amount>,
    pub paid_to_beneficiaries: Vec<Amount>,
    pub epochs_paid: i64,
    pub epochs_closed_unpaid: i64,
    pub epochs_open: i64,
    pub checks: Vec<CheckSummary>,
    pub recent: Vec<DonationBrief>,
}

pub fn overview(conn: &Connection, tokens: &Tokens) -> Result<Overview> {
    let mut donated: BTreeMap<String, u128> = BTreeMap::new();
    let mut donation_count = 0;
    {
        let mut stmt = conn.prepare("SELECT mint, gross_amount FROM donations")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            *donated.entry(r.get(0)?).or_default() += base_units(r.get(1)?);
            donation_count += 1;
        }
    }
    let mut paid: BTreeMap<String, u128> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT p.settlement_mint, l.amount FROM payout_lines l JOIN epoch_payouts p USING (signature, ordinal)
             WHERE l.share_type = 'Beneficiary'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            *paid.entry(r.get(0)?).or_default() += base_units(r.get(1)?);
        }
    }
    let one = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(sql, [], |r| r.get(0)) };
    let epochs_paid = one("SELECT COUNT(*) FROM epoch_payouts")?;
    let epochs_closed_unpaid = one(
        "SELECT COUNT(*) FROM epoch_closes c WHERE NOT EXISTS
         (SELECT 1 FROM epoch_payouts p WHERE p.config_id = c.config_id AND p.mint = c.mint AND p.epoch = c.epoch)",
    )?;
    let epochs_open = one(
        "SELECT COUNT(*) FROM (SELECT DISTINCT config_id, mint, epoch FROM donations) d WHERE NOT EXISTS
         (SELECT 1 FROM epoch_closes c WHERE c.config_id = d.config_id AND c.mint = d.mint AND c.epoch = d.epoch)",
    )?;
    // Largest totals first.
    let mut donated: Vec<(String, u128)> = donated.into_iter().collect();
    donated.sort_by_key(|(m, v)| (known::symbol(m).is_none(), std::cmp::Reverse(*v)));
    Ok(Overview {
        indexing: indexing(conn)?,
        donation_count,
        donated: donated.iter().map(|(m, v)| tokens.amount(m, *v)).collect(),
        paid_to_beneficiaries: paid.iter().map(|(m, v)| tokens.amount(m, *v)).collect(),
        epochs_paid,
        epochs_closed_unpaid,
        epochs_open,
        checks: check_summary(conn)?,
        recent: donations(
            conn,
            tokens,
            "ORDER BY d.slot DESC, d.signature, d.ordinal LIMIT 25",
            &[],
        )?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Found {
    Transaction { signature: String },
    Config { config_id: String },
    Address { address: String },
    Nothing,
}

/// Works out what a search string refers to.
pub fn search(conn: &Connection, query: &str) -> Result<Found> {
    let q = query.trim();
    if let Ok(bytes) = bs58::decode(q).into_vec()
        && bytes.len() == 64
    {
        let known: bool = conn
            .query_row(
                "SELECT 1 FROM transactions WHERE signature = ?1",
                [q],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        return Ok(if known {
            Found::Transaction {
                signature: q.into(),
            }
        } else {
            Found::Nothing
        });
    }
    if let Ok(id) = q.parse::<Bytes32>() {
        let hex = id.to_hex();
        let is_config = conn
            .query_row(
                "SELECT 1 FROM donations WHERE config_id = ?1 LIMIT 1",
                [&hex],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
            || conn
                .query_row(
                    "SELECT 1 FROM epoch_closes WHERE config_id = ?1 LIMIT 1",
                    [&hex],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
        if is_config {
            return Ok(Found::Config { config_id: hex });
        }
        if let Ok(key) = q.parse::<Pubkey>() {
            return Ok(Found::Address {
                address: key.to_string(),
            });
        }
    }
    Ok(Found::Nothing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn loaded() -> (Store, Tokens) {
        let store = Store::with_fixtures();
        let tokens = Tokens::load(store.connection()).unwrap();
        (store, tokens)
    }

    #[test]
    fn a_donation_transaction_resolves_to_its_epoch() {
        let (store, tokens) = loaded();
        let c = store.connection();
        let sig = "2GBt2ohpkrS5r61cUervQpbQPCRhLAW6RUAFAsa2JUvgPRtkeSKN6qC4RLa1VMq2Re2C6eodv9xcKrQD3ycvfXtT";
        let tx = transaction(c, &tokens, sig).unwrap().unwrap();
        assert!(tx.succeeded);
        let [d] = tx.donations.as_slice() else {
            panic!("one donation")
        };
        assert_eq!(d.amount.raw, "73036205764");
        assert_eq!(d.amount.symbol, Some("SOL"));
        assert_eq!(d.amount.decimals, Some(9));
        assert_eq!(d.caller, Some("pump.fun fees"));
        let e = epoch(c, &tokens, &d.pool.config_id, &d.pool.mint, d.pool.epoch)
            .unwrap()
            .unwrap();
        assert_eq!(e.donation_count, 1);
        assert_eq!(e.status, "open", "its close is not a fixture");
    }

    #[test]
    fn the_bridged_epoch_has_its_payout_lines_and_a_share() {
        let (store, tokens) = loaded();
        let c = store.connection();
        let config = "0xb02173170bada3eb8162396b27744f9c5ac99bed3a632b1c4d2bc9b1b366b0e8";
        let e = epoch(c, &tokens, config, known::USDC, 1).unwrap().unwrap();
        assert_eq!(e.status, "paid");
        let payout = e.payout.as_ref().unwrap();
        assert_eq!(payout.settled.raw, "1860005");
        let lines: Vec<(&str, &str)> = payout
            .lines
            .iter()
            .map(|l| (l.share_type.as_str(), l.amount.raw.as_str()))
            .collect();
        assert_eq!(lines, [("Beneficiary", "1804206"), ("Fee", "55799")]);
        // The one fixture donation (932,111 of 1,860,005) gets that fraction of each line, rounded down.
        let s = share(&e.donations[0], &e).unwrap();
        assert_eq!(s.of_epoch, "932111/1860005");
        let est: Vec<&str> = s.lines.iter().map(|l| l.amount.raw.as_str()).collect();
        assert_eq!(est, ["904148", "27962"]);
    }

    #[test]
    fn relayer_transactions_list_the_epochs_they_touched() {
        let (store, tokens) = loaded();
        let tx = transaction(
            store.connection(),
            &tokens,
            "3BpzeRQe9zWQrLxXzHTa7ubBdA91zLxp3WyTHcfMcMKc9LAJfghJes23JGTK46c4AhiwbjXcz7Yad3CfrgkuKqcn",
        )
        .unwrap()
        .unwrap();
        assert!(tx.donations.is_empty());
        let [t] = tx.touched.as_slice() else {
            panic!("one epoch")
        };
        assert_eq!((t.action, t.pool.epoch), ("paid", 1));
    }

    #[test]
    fn search_tells_signatures_configs_and_addresses_apart() {
        let (store, _) = loaded();
        let c = store.connection();
        let sig = "3BpzeRQe9zWQrLxXzHTa7ubBdA91zLxp3WyTHcfMcMKc9LAJfghJes23JGTK46c4AhiwbjXcz7Yad3CfrgkuKqcn";
        assert_eq!(
            search(c, sig).unwrap(),
            Found::Transaction {
                signature: sig.into()
            }
        );
        let config = "0xb02173170bada3eb8162396b27744f9c5ac99bed3a632b1c4d2bc9b1b366b0e8";
        assert_eq!(
            search(c, config).unwrap(),
            Found::Config {
                config_id: config.into()
            }
        );
        assert_eq!(
            search(c, "FdYJBxNVcKwa5JmoCxV8T1c7gGXaERrKLYxxkXFwnR2M").unwrap(),
            Found::Address {
                address: "FdYJBxNVcKwa5JmoCxV8T1c7gGXaERrKLYxxkXFwnR2M".into()
            }
        );
        assert_eq!(search(c, "not a thing").unwrap(), Found::Nothing);
    }

    #[test]
    fn overview_and_config_add_up() {
        let (store, tokens) = loaded();
        let c = store.connection();
        let o = overview(c, &tokens).unwrap();
        assert_eq!(o.donation_count, 4);
        assert_eq!(o.epochs_paid, 5);
        let config = config(
            c,
            &tokens,
            "0xb02173170bada3eb8162396b27744f9c5ac99bed3a632b1c4d2bc9b1b366b0e8",
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.paid_to_beneficiaries[0].raw, "1804206");
        assert_eq!(config.epochs[0].status, "paid");
        let a = address(c, &tokens, "FdYJBxNVcKwa5JmoCxV8T1c7gGXaERrKLYxxkXFwnR2M").unwrap();
        assert!(a.received_count >= 2);
    }
}
