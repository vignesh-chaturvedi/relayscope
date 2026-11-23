//! SQLite storage: the signature work queue, raw transactions, and what they decode to.
//!
//! Every transaction is written inside one SQLite transaction that first deletes anything
//! stored for its signature, so processing the same transaction twice never duplicates a row.
//! Effects (donations, closes, swaps, payouts) are written only for transactions that
//! succeeded; a failed transaction changed nothing on chain.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use relayscope_core::reconcile::{
    CheckResult, CloseRow, DonationRow, EpochData, LineRow, PayoutRow, SwapRow,
};
use relayscope_core::types::ShareType;
use relayscope_core::{DecodedTx, Event};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// How a transaction reached the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Backfill,
    Live,
    Replay,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::Backfill => "backfill",
            Origin::Live => "live",
            Origin::Replay => "replay",
        }
    }
}

/// A row from `getSignaturesForAddress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureRow {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub failed: bool,
}

/// Fetch status of a listed signature.
const UNFETCHED: i64 = 0;
const FETCHED: i64 = 1;
/// Fetched and stored raw, but the decoder rejected it. `reindex` retries these.
const DECODE_FAILED: i64 = 2;

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE signatures (
        signature   TEXT PRIMARY KEY,
        slot        INTEGER NOT NULL,
        block_time  INTEGER,
        failed      INTEGER NOT NULL,
        status      INTEGER NOT NULL DEFAULT 0
    ) WITHOUT ROWID;
    CREATE INDEX signatures_queue ON signatures (status, slot);

    CREATE TABLE transactions (
        signature   TEXT PRIMARY KEY,
        slot        INTEGER NOT NULL,
        block_time  INTEGER,
        succeeded   INTEGER NOT NULL,
        relay_error TEXT,
        fee_payer   TEXT NOT NULL,
        origin      TEXT NOT NULL,
        indexed_at  INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX transactions_slot ON transactions (slot);

    CREATE TABLE raw_transactions (
        signature   TEXT PRIMARY KEY,
        json_zstd   BLOB NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE instructions (
        signature    TEXT NOT NULL,
        ordinal      INTEGER NOT NULL,
        top_index    INTEGER NOT NULL,
        inner_index  INTEGER,
        stack_height INTEGER NOT NULL,
        name         TEXT NOT NULL,
        invoked_by   TEXT,
        config_id    TEXT,
        args         TEXT NOT NULL,
        accounts     TEXT NOT NULL,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;
    CREATE INDEX instructions_name ON instructions (name);

    CREATE TABLE donations (
        signature    TEXT NOT NULL,
        ordinal      INTEGER NOT NULL,
        kind         TEXT NOT NULL,
        config_id    TEXT NOT NULL,
        mint         TEXT NOT NULL,
        epoch        INTEGER NOT NULL,
        gross_amount INTEGER NOT NULL,
        tip          INTEGER NOT NULL,
        message      TEXT,
        credited_to  TEXT,
        donor        TEXT,
        invoked_by   TEXT,
        instruction  TEXT,
        source_id    TEXT,
        payment_id   TEXT,
        slot         INTEGER NOT NULL,
        block_time   INTEGER,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;
    CREATE INDEX donations_pool ON donations (config_id, mint, epoch);

    CREATE TABLE epoch_closes (
        signature          TEXT NOT NULL,
        ordinal            INTEGER NOT NULL,
        config_id          TEXT NOT NULL,
        mint               TEXT NOT NULL,
        epoch              INTEGER NOT NULL,
        gross_epoch_amount INTEGER NOT NULL,
        distribution       TEXT,
        slot               INTEGER NOT NULL,
        block_time         INTEGER,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;
    CREATE INDEX epoch_closes_pool ON epoch_closes (config_id, mint, epoch);

    CREATE TABLE epoch_swaps (
        signature            TEXT NOT NULL,
        ordinal              INTEGER NOT NULL,
        config_id            TEXT NOT NULL,
        mint                 TEXT NOT NULL,
        epoch                INTEGER NOT NULL,
        distribution         TEXT NOT NULL,
        swap_receipt         TEXT NOT NULL,
        output_token_account TEXT NOT NULL,
        swap_output_amount   INTEGER NOT NULL,
        settlement_mint      TEXT NOT NULL,
        slot                 INTEGER NOT NULL,
        block_time           INTEGER,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;
    CREATE INDEX epoch_swaps_pool ON epoch_swaps (config_id, mint, epoch);

    CREATE TABLE epoch_payouts (
        signature       TEXT NOT NULL,
        ordinal         INTEGER NOT NULL,
        config_id       TEXT NOT NULL,
        mint            TEXT NOT NULL,
        epoch           INTEGER NOT NULL,
        generation      TEXT NOT NULL,
        instruction     TEXT,
        settled_amount  INTEGER NOT NULL,
        settlement_mint TEXT NOT NULL,
        slot            INTEGER NOT NULL,
        block_time      INTEGER,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;
    CREATE INDEX epoch_payouts_pool ON epoch_payouts (config_id, mint, epoch);

    CREATE TABLE payout_lines (
        signature  TEXT NOT NULL,
        ordinal    INTEGER NOT NULL,
        line       INTEGER NOT NULL,
        share_type TEXT NOT NULL,
        address    TEXT NOT NULL,
        amount     INTEGER NOT NULL,
        bps        INTEGER,
        PRIMARY KEY (signature, ordinal, line)
    ) WITHOUT ROWID;
    CREATE INDEX payout_lines_address ON payout_lines (address);

    CREATE TABLE bridge_recoveries (
        signature         TEXT NOT NULL,
        ordinal           INTEGER NOT NULL,
        source_id         TEXT NOT NULL,
        payment_id        TEXT NOT NULL,
        mint              TEXT NOT NULL,
        amount            INTEGER NOT NULL,
        destination_owner TEXT NOT NULL,
        authority         TEXT NOT NULL,
        slot              INTEGER NOT NULL,
        block_time        INTEGER,
        PRIMARY KEY (signature, ordinal)
    ) WITHOUT ROWID;

    CREATE TABLE token_changes (
        signature TEXT NOT NULL,
        account   TEXT NOT NULL,
        mint      TEXT NOT NULL,
        owner     TEXT,
        decimals  INTEGER NOT NULL,
        pre       INTEGER NOT NULL,
        post      INTEGER NOT NULL,
        PRIMARY KEY (signature, account)
    ) WITHOUT ROWID;
    CREATE INDEX token_changes_owner ON token_changes (owner, mint);

    CREATE TABLE decode_failures (
        signature TEXT PRIMARY KEY,
        error     TEXT NOT NULL,
        seen_at   INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE state (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;
    "#,
    // 2: reconciliation results
    r#"
    CREATE TABLE checks (
        config_id  TEXT NOT NULL,
        mint       TEXT NOT NULL,
        epoch      INTEGER NOT NULL,
        check_id   TEXT NOT NULL,
        outcome    TEXT NOT NULL,
        expected   TEXT,
        actual     TEXT,
        detail     TEXT NOT NULL,
        checked_at INTEGER NOT NULL,
        PRIMARY KEY (config_id, mint, epoch, check_id)
    ) WITHOUT ROWID;
    CREATE INDEX checks_outcome ON checks (check_id, outcome);
    "#,
];

/// Tables holding what a transaction decoded to, cleared before it is rewritten.
const DERIVED_TABLES: &[&str] = &[
    "transactions",
    "instructions",
    "donations",
    "epoch_closes",
    "epoch_swaps",
    "epoch_payouts",
    "payout_lines",
    "bridge_recoveries",
    "token_changes",
    "decode_failures",
];

pub struct Store {
    conn: Connection,
}

fn int(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| anyhow!("{what} {value} does not fit in a SQLite integer"))
}

fn epoch(value: u128) -> Result<i64> {
    i64::try_from(value).map_err(|_| anyhow!("epoch {value} does not fit in a SQLite integer"))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: usize = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)
                .with_context(|| format!("migration {}", i + 1))?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn state(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Adds listed signatures to the work queue. Returns how many were new.
    pub fn insert_signatures(&mut self, rows: &[SignatureRow]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut added = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO signatures (signature, slot, block_time, failed)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for row in rows {
                added += stmt.execute(params![
                    row.signature,
                    int(row.slot, "slot")?,
                    row.block_time,
                    row.failed
                ])?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// The oldest signatures not yet fetched.
    pub fn unfetched(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT signature FROM signatures WHERE status = ?1 ORDER BY slot, signature LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![UNFETCHED, limit], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn count_unfetched(&self) -> Result<u64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM signatures WHERE status = ?1",
            [UNFETCHED],
            |r| r.get(0),
        )?)
    }

    /// Writes fetched transactions: raw JSON, and what each decoded to or why it failed.
    pub fn write_fetched(&mut self, items: &[Fetched], origin: Origin) -> Result<()> {
        let tx = self.conn.transaction()?;
        for item in items {
            let json = serde_json::to_vec(&item.raw)?;
            let compressed = zstd::encode_all(json.as_slice(), 3)?;
            tx.execute(
                "INSERT INTO raw_transactions (signature, json_zstd) VALUES (?1, ?2)
                 ON CONFLICT (signature) DO UPDATE SET json_zstd = excluded.json_zstd",
                params![item.signature, compressed],
            )?;
            match &item.decoded {
                Ok(decoded) => write_decoded(&tx, decoded, origin)?,
                Err(error) => write_failure(&tx, &item.signature, error)?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Re-decodes every stored raw transaction with the current decoder, without fetching.
    /// With `failed_only`, re-decodes only the transactions the decoder rejected before.
    pub fn reindex(
        &mut self,
        failed_only: bool,
        decode: impl Fn(&serde_json::Value) -> Result<DecodedTx>,
    ) -> Result<ReindexReport> {
        let signatures: Vec<(String, Vec<u8>, String)> = {
            let filter = if failed_only {
                "WHERE r.signature IN (SELECT signature FROM decode_failures)"
            } else {
                ""
            };
            let mut stmt = self.conn.prepare(&format!(
                "SELECT r.signature, r.json_zstd, COALESCE(t.origin, 'backfill')
                 FROM raw_transactions r LEFT JOIN transactions t USING (signature) {filter}"
            ))?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut report = ReindexReport::default();
        let tx = self.conn.transaction()?;
        for (signature, compressed, origin) in signatures {
            let raw: serde_json::Value =
                serde_json::from_slice(&zstd::decode_all(compressed.as_slice())?)?;
            let origin = match origin.as_str() {
                "live" => Origin::Live,
                "replay" => Origin::Replay,
                _ => Origin::Backfill,
            };
            match decode(&raw) {
                Ok(decoded) => {
                    write_decoded(&tx, &decoded, origin)?;
                    report.decoded += 1;
                }
                Err(error) => {
                    write_failure(&tx, &signature, &error.to_string())?;
                    report.failed += 1;
                }
            }
        }
        tx.commit()?;
        Ok(report)
    }
}

/// What C8 needs to know about the signature queue.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Completeness {
    pub history_complete: bool,
    pub listed: u64,
    pub queued: u64,
    pub decode_failures: u64,
}

/// A transaction fetched from RPC, and its decoding result.
pub struct Fetched {
    pub signature: String,
    pub raw: serde_json::Value,
    pub decoded: std::result::Result<DecodedTx, String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReindexReport {
    pub decoded: u64,
    pub failed: u64,
}

fn clear(tx: &Transaction<'_>, signature: &str) -> Result<()> {
    for table in DERIVED_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE signature = ?1"),
            [signature],
        )?;
    }
    Ok(())
}

fn write_failure(tx: &Transaction<'_>, signature: &str, error: &str) -> Result<()> {
    clear(tx, signature)?;
    tx.execute(
        "INSERT INTO decode_failures (signature, error, seen_at) VALUES (?1, ?2, ?3)",
        params![signature, error, now()],
    )?;
    tx.execute(
        "UPDATE signatures SET status = ?2 WHERE signature = ?1",
        params![signature, DECODE_FAILED],
    )?;
    Ok(())
}

fn write_decoded(tx: &Transaction<'_>, d: &DecodedTx, origin: Origin) -> Result<()> {
    let sig = d.signature.as_str();
    let slot = int(d.slot, "slot")?;
    clear(tx, sig)?;

    tx.execute(
        "INSERT INTO transactions (signature, slot, block_time, succeeded, relay_error, fee_payer, origin, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            sig,
            slot,
            d.block_time,
            d.succeeded,
            d.program_error.map(|e| e.name),
            d.fee_payer.to_string(),
            origin.as_str(),
            now()
        ],
    )?;

    for (ordinal, ix) in d.instructions.iter().enumerate() {
        let args = serde_json::to_value(&ix.instruction)?
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tx.execute(
            "INSERT INTO instructions (signature, ordinal, top_index, inner_index, stack_height, name, invoked_by, config_id, args, accounts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                sig,
                ordinal as i64,
                ix.position.top,
                ix.position.inner,
                ix.stack_height,
                ix.instruction.name(),
                ix.invoked_by.map(|p| p.to_string()),
                ix.instruction.config_id().map(|c| c.to_hex()),
                args.to_string(),
                serde_json::to_string(&ix.accounts)?,
            ],
        )?;
    }

    // Unreadable calls in a failed transaction, after the decoded instructions.
    for (i, call) in d.unrecognized.iter().enumerate() {
        tx.execute(
            "INSERT INTO instructions (signature, ordinal, top_index, inner_index, stack_height, name, invoked_by, config_id, args, accounts)
             VALUES (?1, ?2, ?3, ?4, 0, 'unrecognized', NULL, NULL, ?5, '[]')",
            params![
                sig,
                (d.instructions.len() + i) as i64,
                call.position.top,
                call.position.inner,
                serde_json::to_string(call)?,
            ],
        )?;
    }

    for change in &d.token_balances {
        if change.pre == change.post {
            continue;
        }
        tx.execute(
            "INSERT INTO token_changes (signature, account, mint, owner, decimals, pre, post)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sig,
                change.account.to_string(),
                change.mint.to_string(),
                change.owner.map(|o| o.to_string()),
                change.decimals,
                int(change.pre, "balance")?,
                int(change.post, "balance")?,
            ],
        )?;
    }

    if !d.succeeded {
        tx.execute(
            "UPDATE signatures SET status = ?2 WHERE signature = ?1",
            params![sig, FETCHED],
        )?;
        return Ok(());
    }

    for (ordinal, event) in d.events.iter().enumerate() {
        let ordinal = ordinal as i64;
        let emitter = d.emitter(event);
        let emitter_name = emitter.map(|i| i.instruction.name());
        match &event.event {
            Event::DonationMadeV1Event(e) => {
                let donor = emitter
                    .and_then(|i| i.account("from"))
                    .map(|k| k.to_string());
                tx.execute(
                    "INSERT INTO donations (signature, ordinal, kind, config_id, mint, epoch, gross_amount, tip, message,
                                            credited_to, donor, invoked_by, instruction, slot, block_time)
                     VALUES (?1, ?2, 'direct', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        sig,
                        ordinal,
                        e.config_id.to_hex(),
                        e.mint.to_string(),
                        epoch(e.epoch)?,
                        int(e.gross_amount, "gross_amount")?,
                        int(e.tip, "tip")?,
                        e.message,
                        e.credited_to.to_string(),
                        donor,
                        emitter.and_then(|i| i.invoked_by).map(|p| p.to_string()),
                        emitter_name,
                        slot,
                        d.block_time,
                    ],
                )?;
            }
            Event::BridgeDonationAdmittedV1Event(e) => {
                tx.execute(
                    "INSERT INTO donations (signature, ordinal, kind, config_id, mint, epoch, gross_amount, tip,
                                            invoked_by, instruction, source_id, payment_id, slot, block_time)
                     VALUES (?1, ?2, 'bridge', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        sig,
                        ordinal,
                        e.config_id.to_hex(),
                        e.mint.to_string(),
                        epoch(e.epoch)?,
                        int(e.gross_amount, "gross_amount")?,
                        int(e.tip, "tip")?,
                        emitter.and_then(|i| i.invoked_by).map(|p| p.to_string()),
                        emitter_name,
                        e.source_id.to_hex(),
                        e.payment_id.to_hex(),
                        slot,
                        d.block_time,
                    ],
                )?;
            }
            Event::DonationEpochClosedV1Event(e) => {
                tx.execute(
                    "INSERT INTO epoch_closes (signature, ordinal, config_id, mint, epoch, gross_epoch_amount, distribution, slot, block_time)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        sig,
                        ordinal,
                        e.config_id.to_hex(),
                        e.mint.to_string(),
                        epoch(e.epoch)?,
                        int(e.gross_epoch_amount, "gross_epoch_amount")?,
                        emitter.and_then(|i| i.account("distribution")).map(|k| k.to_string()),
                        slot,
                        d.block_time,
                    ],
                )?;
            }
            Event::DonationEpochJustSwappedV1Event(e) => {
                tx.execute(
                    "INSERT INTO epoch_swaps (signature, ordinal, config_id, mint, epoch, distribution, swap_receipt,
                                              output_token_account, swap_output_amount, settlement_mint, slot, block_time)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        sig,
                        ordinal,
                        e.config_id.to_hex(),
                        e.mint.to_string(),
                        epoch(e.epoch)?,
                        e.distribution.to_string(),
                        e.swap_receipt.to_string(),
                        e.output_token_account.to_string(),
                        int(e.swap_output_amount, "swap_output_amount")?,
                        e.settlement_mint.to_string(),
                        slot,
                        d.block_time,
                    ],
                )?;
            }
            Event::DonationEpochProcessedExactV1Event(e) => {
                insert_payout(
                    tx,
                    sig,
                    ordinal,
                    slot,
                    d.block_time,
                    emitter_name,
                    "exact",
                    &e.config_id.to_hex(),
                    &e.mint.to_string(),
                    epoch(e.epoch)?,
                    int(e.settled_amount, "settled_amount")?,
                    &e.settlement_mint.to_string(),
                )?;
                for (line, t) in e.transfers.iter().enumerate() {
                    insert_line(
                        tx,
                        sig,
                        ordinal,
                        line,
                        &format!("{:?}", t.share_type),
                        &t.address.to_string(),
                        int(t.amount, "amount")?,
                        None,
                    )?;
                }
            }
            Event::DonationEpochProcessedV1Event(e) => {
                insert_payout(
                    tx,
                    sig,
                    ordinal,
                    slot,
                    d.block_time,
                    emitter_name,
                    "bps",
                    &e.config_id.to_hex(),
                    &e.mint.to_string(),
                    epoch(e.epoch)?,
                    int(e.settled_amount, "settled_amount")?,
                    &e.settlement_mint.to_string(),
                )?;
                for (line, t) in e.transfers.iter().enumerate() {
                    insert_line(
                        tx,
                        sig,
                        ordinal,
                        line,
                        &format!("{:?}", t.share_type),
                        &t.address.to_string(),
                        int(t.amount, "amount")?,
                        Some(t.bps),
                    )?;
                }
            }
            Event::BridgePaymentRecoveredV1Event(e) => {
                tx.execute(
                    "INSERT INTO bridge_recoveries (signature, ordinal, source_id, payment_id, mint, amount, destination_owner, authority, slot, block_time)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        sig,
                        ordinal,
                        e.source_id.to_hex(),
                        e.payment_id.to_hex(),
                        e.mint.to_string(),
                        int(e.amount, "amount")?,
                        e.destination_owner.to_string(),
                        e.authority.to_string(),
                        slot,
                        d.block_time,
                    ],
                )?;
            }
        }
    }

    tx.execute(
        "UPDATE signatures SET status = ?2 WHERE signature = ?1",
        params![sig, FETCHED],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_payout(
    tx: &Transaction<'_>,
    sig: &str,
    ordinal: i64,
    slot: i64,
    block_time: Option<i64>,
    instruction: Option<&str>,
    generation: &str,
    config_id: &str,
    mint: &str,
    epoch: i64,
    settled_amount: i64,
    settlement_mint: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO epoch_payouts (signature, ordinal, config_id, mint, epoch, generation, instruction, settled_amount, settlement_mint, slot, block_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![sig, ordinal, config_id, mint, epoch, generation, instruction, settled_amount, settlement_mint, slot, block_time],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_line(
    tx: &Transaction<'_>,
    sig: &str,
    ordinal: i64,
    line: usize,
    share_type: &str,
    address: &str,
    amount: i64,
    bps: Option<u16>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO payout_lines (signature, ordinal, line, share_type, address, amount, bps)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![sig, ordinal, line as i64, share_type, address, amount, bps],
    )?;
    Ok(())
}

/// Counts and totals for `relayscope stats`.
#[derive(Debug, Default)]
pub struct Stats {
    pub signatures: u64,
    pub signatures_failed: u64,
    pub fetched: u64,
    pub unfetched: u64,
    pub decode_failures: u64,
    pub transactions: u64,
    pub transactions_failed: u64,
    pub first_block_time: Option<i64>,
    pub last_block_time: Option<i64>,
    pub instructions: BTreeMap<String, u64>,
    pub idl_management: u64,
    pub donations_by_kind: BTreeMap<String, u64>,
    pub donations_by_caller: BTreeMap<String, u64>,
    /// Per mint: number of donations and total gross amount in base units.
    pub donated: BTreeMap<String, (u64, u128)>,
    pub configs: u64,
    pub closes: u64,
    pub swaps: u64,
    pub payouts_by_generation: BTreeMap<String, u64>,
    /// Per settlement mint and share type: total paid in base units.
    pub paid: BTreeMap<(String, String), u128>,
    pub decimals: BTreeMap<String, u8>,
}

impl Store {
    pub fn stats(&self) -> Result<Stats> {
        let c = &self.conn;
        let count = |sql: &str| -> Result<u64> { Ok(c.query_row(sql, [], |r| r.get(0))?) };
        let mut s = Stats {
            signatures: count("SELECT COUNT(*) FROM signatures")?,
            signatures_failed: count("SELECT COUNT(*) FROM signatures WHERE failed")?,
            fetched: count("SELECT COUNT(*) FROM signatures WHERE status <> 0")?,
            unfetched: count("SELECT COUNT(*) FROM signatures WHERE status = 0")?,
            decode_failures: count("SELECT COUNT(*) FROM decode_failures")?,
            transactions: count("SELECT COUNT(*) FROM transactions")?,
            transactions_failed: count("SELECT COUNT(*) FROM transactions WHERE NOT succeeded")?,
            configs: count("SELECT COUNT(DISTINCT config_id) FROM donations")?,
            closes: count("SELECT COUNT(*) FROM epoch_closes")?,
            swaps: count("SELECT COUNT(*) FROM epoch_swaps")?,
            ..Stats::default()
        };
        (s.first_block_time, s.last_block_time) = c.query_row(
            "SELECT MIN(block_time), MAX(block_time) FROM transactions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let grouped = |sql: &str| -> Result<BTreeMap<String, u64>> {
            let mut stmt = c.prepare(sql)?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?
                        .unwrap_or_else(|| "direct call".into()),
                    r.get(1)?,
                ))
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        };
        s.instructions = grouped("SELECT name, COUNT(*) FROM instructions GROUP BY name")?;
        s.donations_by_kind = grouped("SELECT kind, COUNT(*) FROM donations GROUP BY kind")?;
        s.donations_by_caller =
            grouped("SELECT invoked_by, COUNT(*) FROM donations GROUP BY invoked_by")?;
        s.payouts_by_generation =
            grouped("SELECT generation, COUNT(*) FROM epoch_payouts GROUP BY generation")?;
        s.idl_management = count(
            "SELECT COUNT(*) FROM transactions t WHERE NOT EXISTS (SELECT 1 FROM instructions i WHERE i.signature = t.signature)
             AND NOT EXISTS (SELECT 1 FROM signatures g WHERE g.signature = t.signature AND g.failed)",
        )?;

        // Totals are summed in Rust as u128, so they cannot overflow.
        {
            let mut stmt = c.prepare("SELECT mint, gross_amount FROM donations")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let entry = s.donated.entry(row.get(0)?).or_default();
                entry.0 += 1;
                entry.1 += u128::from(row.get::<_, i64>(1)? as u64);
            }
        }
        {
            let mut stmt = c.prepare(
                "SELECT p.settlement_mint, l.share_type, l.amount FROM payout_lines l
                 JOIN epoch_payouts p USING (signature, ordinal)",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                *s.paid.entry((row.get(0)?, row.get(1)?)).or_default() +=
                    u128::from(row.get::<_, i64>(2)? as u64);
            }
        }
        {
            let mut stmt =
                c.prepare("SELECT mint, MAX(decimals) FROM token_changes GROUP BY mint")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            s.decimals = rows.collect::<Result<_, _>>()?;
        }
        Ok(s)
    }

    /// Everything recorded for every epoch, from the stored rows, ready for reconciliation.
    pub fn load_epochs(&self) -> Result<Vec<EpochData>> {
        type Key = (String, String, i64);
        let c = &self.conn;
        let mut epochs: BTreeMap<Key, EpochData> = BTreeMap::new();
        fn entry(map: &mut BTreeMap<Key, EpochData>, key: Key) -> &mut EpochData {
            map.entry(key.clone()).or_insert_with(|| EpochData {
                config_id: key.0,
                mint: key.1,
                epoch: u128::try_from(key.2).unwrap_or_default(),
                ..EpochData::default()
            })
        }
        let amount = |v: i64| u64::try_from(v).unwrap_or_default();

        let mut stmt = c.prepare(
            "SELECT config_id, mint, epoch, signature, gross_amount, tip, block_time FROM donations",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            entry(&mut epochs, (r.get(0)?, r.get(1)?, r.get(2)?))
                .donations
                .push(DonationRow {
                    signature: r.get(3)?,
                    gross: amount(r.get(4)?),
                    tip: amount(r.get(5)?),
                    block_time: r.get(6)?,
                });
        }

        let mut stmt = c.prepare(
            "SELECT config_id, mint, epoch, signature, gross_epoch_amount, block_time FROM epoch_closes",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            entry(&mut epochs, (r.get(0)?, r.get(1)?, r.get(2)?))
                .closes
                .push(CloseRow {
                    signature: r.get(3)?,
                    gross_epoch: amount(r.get(4)?),
                    block_time: r.get(5)?,
                });
        }

        let mut stmt = c.prepare(
            "SELECT s.config_id, s.mint, s.epoch, s.signature, s.swap_output_amount, t.post - t.pre, s.block_time
             FROM epoch_swaps s
             LEFT JOIN token_changes t ON t.signature = s.signature AND t.account = s.output_token_account",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            entry(&mut epochs, (r.get(0)?, r.get(1)?, r.get(2)?))
                .swaps
                .push(SwapRow {
                    signature: r.get(3)?,
                    output: amount(r.get(4)?),
                    output_received: r.get::<_, Option<i64>>(5)?.map(i128::from),
                    block_time: r.get(6)?,
                });
        }

        // Payout lines, and what each owner received in the settlement mint, per payout transaction.
        let mut lines: BTreeMap<(String, i64), Vec<LineRow>> = BTreeMap::new();
        let mut stmt = c.prepare(
            "SELECT signature, ordinal, share_type, address, amount, bps FROM payout_lines
             ORDER BY signature, ordinal, line",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let share_type = match r.get::<_, String>(2)?.as_str() {
                "Beneficiary" => ShareType::Beneficiary,
                "Fee" => ShareType::Fee,
                "Tip" => ShareType::Tip,
                other => return Err(anyhow!("unknown share type {other}")),
            };
            lines
                .entry((r.get(0)?, r.get(1)?))
                .or_default()
                .push(LineRow {
                    share_type,
                    address: r.get(3)?,
                    amount: amount(r.get(4)?),
                    bps: r.get::<_, Option<u16>>(5)?,
                });
        }
        let mut received: BTreeMap<String, BTreeMap<String, i128>> = BTreeMap::new();
        let mut stmt = c.prepare(
            "SELECT t.signature, t.owner, SUM(t.post - t.pre) FROM token_changes t
             JOIN (SELECT DISTINCT signature, settlement_mint FROM epoch_payouts) p
               ON p.signature = t.signature AND p.settlement_mint = t.mint
             WHERE t.owner IS NOT NULL
             GROUP BY t.signature, t.owner",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            received
                .entry(r.get(0)?)
                .or_default()
                .insert(r.get(1)?, i128::from(r.get::<_, i64>(2)?));
        }

        let mut stmt = c.prepare(
            "SELECT config_id, mint, epoch, signature, ordinal, generation, instruction, settled_amount,
                    settlement_mint, block_time
             FROM epoch_payouts",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let signature: String = r.get(3)?;
            let ordinal: i64 = r.get(4)?;
            entry(&mut epochs, (r.get(0)?, r.get(1)?, r.get(2)?))
                .payouts
                .push(PayoutRow {
                    instruction: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    exact: r.get::<_, String>(5)? == "exact",
                    settled: amount(r.get(7)?),
                    settlement_mint: r.get(8)?,
                    lines: lines
                        .remove(&(signature.clone(), ordinal))
                        .unwrap_or_default(),
                    received: received.get(&signature).cloned().unwrap_or_default(),
                    block_time: r.get(9)?,
                    signature,
                });
        }
        Ok(epochs.into_values().collect())
    }

    /// Replaces every stored check result.
    pub fn write_checks(&mut self, results: &[(&EpochData, Vec<CheckResult>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM checks", [])?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO checks (config_id, mint, epoch, check_id, outcome, expected, actual, detail, checked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let at = now();
            for (epoch, checks) in results {
                let epoch_number = i64::try_from(epoch.epoch).unwrap_or(i64::MAX);
                for r in checks {
                    stmt.execute(params![
                        epoch.config_id,
                        epoch.mint,
                        epoch_number,
                        r.check.id(),
                        r.outcome.as_str(),
                        r.expected,
                        r.actual,
                        r.detail,
                        at
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// For C8: whether listing reached the program's first transaction, and how many listed
    /// signatures are still queued or were rejected by the decoder.
    pub fn completeness(&self) -> Result<Completeness> {
        let c = &self.conn;
        Ok(Completeness {
            history_complete: self.state("history_complete")?.is_some(),
            listed: c.query_row("SELECT COUNT(*) FROM signatures", [], |r| r.get(0))?,
            queued: c.query_row(
                "SELECT COUNT(*) FROM signatures WHERE status = ?1",
                [UNFETCHED],
                |r| r.get(0),
            )?,
            decode_failures: c
                .query_row("SELECT COUNT(*) FROM decode_failures", [], |r| r.get(0))?,
        })
    }

    /// Decode failures, newest first.
    pub fn decode_failures(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT signature, error FROM decode_failures ORDER BY seen_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// Decodes one fetched transaction, keeping the error as text if it fails.
pub fn decode_fetched(signature: String, raw: serde_json::Value) -> Fetched {
    let decoded = relayscope_core::TxView::from_rpc_json(&raw)
        .and_then(|view| relayscope_core::decode_transaction(&view))
        .map_err(|e| e.to_string());
    Fetched {
        signature,
        raw,
        decoded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> Vec<(String, serde_json::Value)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let mut out: Vec<(String, serde_json::Value)> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| {
                let raw: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
                let sig = raw["transaction"]["signatures"][0]
                    .as_str()
                    .unwrap()
                    .to_string();
                (sig, raw)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn load_all(store: &mut Store) {
        let rows: Vec<SignatureRow> = fixtures()
            .iter()
            .map(|(sig, raw)| SignatureRow {
                signature: sig.clone(),
                slot: raw["slot"].as_u64().unwrap(),
                block_time: raw["blockTime"].as_i64(),
                failed: !raw["meta"]["err"].is_null(),
            })
            .collect();
        store.insert_signatures(&rows).unwrap();
        let fetched: Vec<Fetched> = fixtures()
            .into_iter()
            .map(|(s, r)| decode_fetched(s, r))
            .collect();
        store.write_fetched(&fetched, Origin::Backfill).unwrap();
    }

    fn table_counts(store: &Store) -> Vec<(String, u64)> {
        [
            "signatures",
            "transactions",
            "raw_transactions",
            "instructions",
            "donations",
            "epoch_closes",
            "epoch_swaps",
            "epoch_payouts",
            "payout_lines",
            "token_changes",
            "decode_failures",
        ]
        .iter()
        .map(|t| {
            let n: u64 = store
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            (t.to_string(), n)
        })
        .collect()
    }

    #[test]
    fn migrations_run_once() {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate().unwrap();
        let version: usize = store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len());
    }

    #[test]
    fn fixtures_load_into_the_expected_rows() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let counts: BTreeMap<String, u64> = table_counts(&store).into_iter().collect();
        assert_eq!(counts["signatures"], 18);
        assert_eq!(counts["transactions"], 18);
        assert_eq!(counts["raw_transactions"], 18);
        assert_eq!(counts["decode_failures"], 0);
        // Donations: native direct, SPL direct, pump.fun CPI, bridge.
        assert_eq!(counts["donations"], 4);
        // Closes: a SOL epoch and the bridged USDC epoch.
        assert_eq!(counts["epoch_closes"], 2);
        assert_eq!(counts["epoch_swaps"], 1);
        // Exact after swap, exact non-swap, bps non-swap, bps after swap, bps combined swap.
        assert_eq!(counts["epoch_payouts"], 5);
        assert_eq!(store.count_unfetched().unwrap(), 0);
    }

    #[test]
    fn writing_twice_changes_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let before = table_counts(&store);
        load_all(&mut store);
        assert_eq!(table_counts(&store), before);
    }

    #[test]
    fn failed_transactions_have_no_effects() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let effects: u64 = store
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM donations d JOIN transactions t USING (signature) WHERE NOT t.succeeded)
                      + (SELECT COUNT(*) FROM epoch_payouts p JOIN transactions t USING (signature) WHERE NOT t.succeeded)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(effects, 0);
        let failed: u64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM transactions WHERE NOT succeeded",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(failed, 4);
    }

    #[test]
    fn donations_record_who_called_the_relay() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let callers: Vec<(String, Option<String>, Option<String>)> = {
            let mut stmt = store
                .conn
                .prepare(
                    "SELECT kind, invoked_by, instruction FROM donations ORDER BY kind, invoked_by",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert!(callers.contains(&(
            "direct".into(),
            Some("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ".into()),
            Some("donate_pubkey_config_id_with_payer_v1".into())
        )));
        assert!(callers.contains(&(
            "bridge".into(),
            None,
            Some("admit_bridge_payment_v1".into())
        )));
    }

    #[test]
    fn stats_add_up_payouts_per_share_type() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let stats = store.stats().unwrap();
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string();
        // Fees from the two exact payouts (32,050,293 and 55,799) plus the three older ones.
        assert!(stats.paid[&(usdc.clone(), "Fee".into())] >= 32_050_293 + 55_799);
        assert_eq!(stats.donations_by_kind["bridge"], 1);
        assert_eq!(stats.idl_management, 1);
        assert_eq!(stats.decimals.get(&usdc), Some(&6));
    }

    #[test]
    fn reindex_rebuilds_from_raw_without_fetching() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let before = table_counts(&store);
        let report = store
            .reindex(false, |raw| {
                let view = relayscope_core::TxView::from_rpc_json(raw)?;
                Ok(relayscope_core::decode_transaction(&view)?)
            })
            .unwrap();
        assert_eq!(
            report,
            ReindexReport {
                decoded: 18,
                failed: 0
            }
        );
        assert_eq!(table_counts(&store), before);
    }

    #[test]
    fn decode_failures_are_recorded_and_kept_raw() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert_signatures(&[SignatureRow {
                signature: "bad".into(),
                slot: 1,
                block_time: None,
                failed: false,
            }])
            .unwrap();
        let item = Fetched {
            signature: "bad".into(),
            raw: serde_json::json!({"not": "a transaction"}),
            decoded: Err("unknown instruction discriminator".into()),
        };
        store.write_fetched(&[item], Origin::Backfill).unwrap();
        assert_eq!(store.decode_failures(10).unwrap().len(), 1);
        assert_eq!(
            store.count_unfetched().unwrap(),
            0,
            "not retried on every run"
        );
        let raw: u64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM raw_transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, 1, "kept for reindex after a decoder fix");
    }

    #[test]
    fn reindex_failed_only_retries_just_the_rejected_ones() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        // Pretend the decoder once rejected one fixture.
        let (signature, raw) = fixtures().into_iter().next().unwrap();
        let rejected = Fetched {
            signature: signature.clone(),
            raw,
            decoded: Err("old decoder bug".into()),
        };
        store.write_fetched(&[rejected], Origin::Backfill).unwrap();
        assert_eq!(store.decode_failures(10).unwrap().len(), 1);

        let report = store
            .reindex(true, |raw| {
                let view = relayscope_core::TxView::from_rpc_json(raw)?;
                Ok(relayscope_core::decode_transaction(&view)?)
            })
            .unwrap();
        assert_eq!(
            report,
            ReindexReport {
                decoded: 1,
                failed: 0
            },
            "only the rejected one"
        );
        assert!(store.decode_failures(10).unwrap().is_empty());
        let status: i64 = store
            .conn
            .query_row(
                "SELECT status FROM signatures WHERE signature = ?1",
                [&signature],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, FETCHED);
    }

    #[test]
    fn unrecognized_calls_are_stored_as_instructions() {
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let n: u64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM instructions WHERE name = 'unrecognized'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn real_fixtures_reconcile_through_the_loader() {
        use relayscope_core::reconcile::{Check, Outcome, reconcile};
        let mut store = Store::open_in_memory().unwrap();
        load_all(&mut store);
        let epochs = store.load_epochs().unwrap();
        let find = |config: &str, epoch: u128| {
            epochs
                .iter()
                .find(|e| e.config_id.starts_with(config) && e.epoch == epoch)
                .unwrap()
        };
        let outcome = |e: &EpochData, check: Check| {
            reconcile(e, 1_800_000_000)
                .into_iter()
                .find(|r| r.check == check)
                .unwrap()
        };

        // The bridged USDC epoch: admission, close and payout are fixtures, but only one of
        // its two donations is. The inflow check sees the gap and gives the numbers.
        let bridged = find("0xb021", 1);
        assert_eq!(outcome(bridged, Check::Settlement).outcome, Outcome::Pass);
        assert_eq!(outcome(bridged, Check::Landed).outcome, Outcome::Pass);
        let inflow = outcome(bridged, Check::Inflow);
        assert_eq!(inflow.outcome, Outcome::Fail);
        assert_eq!(
            (inflow.expected.as_deref(), inflow.actual.as_deref()),
            (Some("932111"), Some("1860005"))
        );
        // With the other donation (1,860,005 - 932,111 = 927,894), the epoch adds up, and its
        // fee is 3% with one unit of rounding per donation: 55,799 against 55,800.15.
        let mut whole = bridged.clone();
        whole
            .donations
            .push(relayscope_core::reconcile::DonationRow {
                signature: "second bridged donation".into(),
                gross: 927_894,
                tip: 0,
                block_time: whole.donations[0].block_time,
            });
        assert_eq!(outcome(&whole, Check::Inflow).outcome, Outcome::Pass);
        assert_eq!(outcome(&whole, Check::FeeRate).detail, "300 bps");

        // The 20% SOL epoch: its payout landed, but its swap is not a fixture, so it fails C3.
        let swapped = find("0x4f01", 2);
        assert_eq!(outcome(swapped, Check::Landed).outcome, Outcome::Pass);
        assert_eq!(outcome(swapped, Check::Settlement).outcome, Outcome::Fail);

        let results: Vec<_> = epochs
            .iter()
            .map(|e| (e, reconcile(e, 1_800_000_000)))
            .collect();
        store.write_checks(&results).unwrap();
        let stored: u64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM checks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, epochs.len() as u64 * 7);
    }

    #[test]
    fn rejects_values_too_large_for_sqlite() {
        assert!(int(u64::MAX, "amount").is_err());
        assert!(epoch(u128::from(u64::MAX)).is_err());
        assert_eq!(int(42, "amount").unwrap(), 42);
    }
}
