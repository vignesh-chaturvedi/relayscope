//! Signed webhooks for new donations, closed and settled epochs, and failed checks.
//!
//! With `relayscope serve --webhooks <file>`, a detector watches what the indexer stores and
//! writes each event once to an outbox in SQLite. A sender then delivers it to every endpoint
//! that subscribed, retrying with backoff for about 22 hours. Requests are signed as the
//! Standard Webhooks spec describes (<https://www.standardwebhooks.com>), so its libraries
//! can verify them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use relayscope_core::Bytes32;
use relayscope_core::reconcile::Check;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::api::{self, Amount, DonationBrief, Payout, Pool, Swap, Tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventType {
    DonationMade,
    EpochClosed,
    EpochSettled,
    CheckFailed,
}

impl EventType {
    pub const ALL: [EventType; 4] = [
        EventType::DonationMade,
        EventType::EpochClosed,
        EventType::EpochSettled,
        EventType::CheckFailed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventType::DonationMade => "donation.made",
            EventType::EpochClosed => "epoch.closed",
            EventType::EpochSettled => "epoch.settled",
            EventType::CheckFailed => "check.failed",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

/// The webhooks file, in TOML.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    public_url: Option<String>,
    #[serde(default, rename = "endpoint")]
    endpoints: Vec<FileEndpoint>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEndpoint {
    url: String,
    secret: Option<String>,
    secret_env: Option<String>,
    events: Option<Vec<String>>,
    configs: Option<Vec<String>>,
}

pub struct Config {
    /// Base URL of this site, for links in payloads.
    pub public_url: Option<String>,
    pub endpoints: Vec<Endpoint>,
}

pub struct Endpoint {
    pub url: String,
    key: Vec<u8>,
    events: HashSet<EventType>,
    /// Config IDs in hex, or `None` for every config.
    configs: Option<HashSet<String>>,
}

impl Endpoint {
    fn wants(&self, kind: EventType, config_id: &str) -> bool {
        self.events.contains(&kind) && self.configs.as_ref().is_none_or(|c| c.contains(config_id))
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text, |name| std::env::var(name).ok())
            .with_context(|| format!("in {}", path.display()))
    }

    fn parse(text: &str, env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let file: FileConfig = toml::from_str(text)?;
        if file.endpoints.is_empty() {
            bail!("no [[endpoint]] sections");
        }
        let mut urls = HashSet::new();
        let mut endpoints = Vec::new();
        for (i, e) in file.endpoints.into_iter().enumerate() {
            let n = i + 1;
            if !(e.url.starts_with("https://") || e.url.starts_with("http://")) {
                bail!("endpoint {n}: the url must start with https:// or http://");
            }
            if !urls.insert(e.url.clone()) {
                bail!("endpoint {n}: {} is listed twice", e.url);
            }
            let secret = match (e.secret, e.secret_env) {
                (Some(secret), None) => secret,
                (None, Some(var)) => env(&var).with_context(|| {
                    format!("endpoint {n}: the environment variable {var} is not set")
                })?,
                _ => bail!("endpoint {n}: set either secret or secret_env"),
            };
            let key = parse_secret(&secret).with_context(|| format!("endpoint {n}"))?;
            let events = match e.events {
                None => EventType::ALL.into_iter().collect(),
                Some(names) => names
                    .iter()
                    .map(|name| {
                        EventType::parse(name).with_context(|| {
                            format!(
                                "endpoint {n}: unknown event {name:?}; the events are {}",
                                EventType::ALL.map(EventType::as_str).join(", ")
                            )
                        })
                    })
                    .collect::<Result<_>>()?,
            };
            let configs = e
                .configs
                .map(|ids| {
                    ids.iter()
                        .map(|id| {
                            id.parse::<Bytes32>()
                                .map(|b| b.to_hex())
                                .map_err(|_| anyhow!("endpoint {n}: {id:?} is not a config ID"))
                        })
                        .collect::<Result<HashSet<_>>>()
                })
                .transpose()?;
            endpoints.push(Endpoint {
                url: e.url,
                key,
                events,
                configs,
            });
        }
        Ok(Self {
            public_url: file.public_url.map(|u| u.trim_end_matches('/').to_string()),
            endpoints,
        })
    }

    fn endpoint(&self, url: &str) -> Option<&Endpoint> {
        self.endpoints.iter().find(|e| e.url == url)
    }

    fn link(&self, path: &str) -> Option<String> {
        self.public_url.as_ref().map(|base| format!("{base}{path}"))
    }
}

/// The HMAC key in a `whsec_` secret.
fn parse_secret(secret: &str) -> Result<Vec<u8>> {
    let encoded = secret
        .strip_prefix("whsec_")
        .context("a secret starts with whsec_; create one with `relayscope webhooks secret`")?;
    let key = BASE64
        .decode(encoded)
        .context("the part after whsec_ is not base64")?;
    if !(24..=64).contains(&key.len()) {
        bail!(
            "a secret should hold 24 to 64 bytes, this one holds {}",
            key.len()
        );
    }
    Ok(key)
}

/// A new random secret, 32 bytes from the operating system.
pub fn new_secret() -> Result<String> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| anyhow!("no randomness from the OS: {e}"))?;
    Ok(format!("whsec_{}", BASE64.encode(key)))
}

/// The `webhook-signature` header value: HMAC-SHA256 over `{id}.{timestamp}.{body}`.
pub fn sign(key: &[u8], id: &str, timestamp: i64, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(format!("{id}.{timestamp}.").as_bytes());
    mac.update(body.as_bytes());
    format!("v1,{}", BASE64.encode(mac.finalize().into_bytes()))
}

/// A stable ID, so finding the same event twice never sends it twice.
fn event_id(kind: EventType, key: &str) -> String {
    let digest = Sha256::digest(format!("{}|{key}", kind.as_str()).as_bytes());
    format!("msg_{}", hex::encode(&digest[..16]))
}

/// Seconds to wait after each failed attempt. Eleven attempts in all, the last about 22 hours
/// after the first.
const RETRY_AFTER: [i64; 10] = [5, 30, 120, 600, 1_800, 3_600, 7_200, 14_400, 21_600, 28_800];

/// How long to wait after `attempts` failed attempts, or `None` to give up.
pub fn retry_after(attempts: u32) -> Option<i64> {
    RETRY_AFTER
        .get(usize::try_from(attempts.checked_sub(1)?).ok()?)
        .copied()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn rfc3339(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    #[serde(rename = "type")]
    kind: &'static str,
    timestamp: String,
    data: &'a T,
}

#[derive(Serialize)]
struct DonationData<'a> {
    #[serde(flatten)]
    donation: &'a DonationBrief,
    url: Option<String>,
}

#[derive(Serialize)]
struct EpochClosedData<'a> {
    pool: &'a Pool,
    signature: &'a str,
    slot: i64,
    block_time: Option<i64>,
    finalized: bool,
    gross: &'a Amount,
    donation_count: usize,
    url: Option<String>,
}

#[derive(Serialize)]
struct EpochSettledData<'a> {
    pool: &'a Pool,
    signature: &'a str,
    slot: i64,
    block_time: Option<i64>,
    finalized: bool,
    donation_count: usize,
    donated: &'a Amount,
    swap: Option<&'a Swap>,
    payout: &'a Payout,
    url: Option<String>,
}

#[derive(Serialize)]
struct CheckFailedData<'a> {
    pool: Pool,
    check: &'a str,
    name: &'static str,
    expected: Option<&'a str>,
    actual: Option<&'a str>,
    detail: &'a str,
    url: Option<String>,
}

struct NewEvent {
    id: String,
    kind: EventType,
    config_id: String,
    body: String,
}

impl NewEvent {
    fn new(
        id: String,
        kind: EventType,
        config_id: &str,
        data: &impl Serialize,
        now: i64,
    ) -> Result<Self> {
        let body = serde_json::to_string(&Envelope {
            kind: kind.as_str(),
            timestamp: rfc3339(now),
            data,
        })?;
        Ok(Self {
            id,
            kind,
            config_id: config_id.to_string(),
            body,
        })
    }
}

fn event_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM webhook_events WHERE id = ?1")?
        .exists([id])?)
}

/// Writes new events and a pending delivery for each endpoint that wants one. Returns how
/// many events were new.
fn write_events(
    conn: &Connection,
    config: &Config,
    events: &[NewEvent],
    now: i64,
) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let mut written = 0;
    for e in events {
        let added = tx.execute(
            "INSERT OR IGNORE INTO webhook_events (id, type, config_id, created_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![e.id, e.kind.as_str(), e.config_id, now, e.body],
        )?;
        if added == 0 {
            continue;
        }
        written += 1;
        for endpoint in config
            .endpoints
            .iter()
            .filter(|x| x.wants(e.kind, &e.config_id))
        {
            tx.execute(
                "INSERT OR IGNORE INTO webhook_deliveries (event_id, endpoint, state, attempts, next_at)
                 VALUES (?1, ?2, 'pending', 0, ?3)",
                params![e.id, endpoint.url, now],
            )?;
        }
    }
    tx.commit()?;
    Ok(written)
}

fn get_state(conn: &Connection, key: &str) -> Result<Option<i64>> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM state WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()))
}

fn set_state(conn: &Connection, key: &str, value: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value.to_string()],
    )?;
    Ok(())
}

const START_SLOT: &str = "webhooks.start_slot";
const CURSOR: &str = "webhooks.cursor";
/// Seconds of `indexed_at` to look at again on each pass, for writes that committed late.
const OVERLAP: i64 = 10;
const CHECK_SCAN_EVERY: Duration = Duration::from_secs(30);

/// Finds new events in what the indexer stored.
pub struct Detector {
    tokens: Tokens,
    tokens_loaded: Instant,
    /// Transactions already looked at, by signature, with the `indexed_at` they had then. A
    /// rewrite (when the finalizer stores a transaction again) changes `indexed_at`.
    seen: HashMap<String, i64>,
    /// Failing checks from the last scan, by event ID, with the `checked_at` first seen.
    failing: HashMap<String, i64>,
    last_check_scan: Option<Instant>,
}

impl Detector {
    pub fn new(conn: &Connection) -> Result<Self> {
        Ok(Self {
            tokens: Tokens::load(conn)?,
            tokens_loaded: Instant::now(),
            seen: HashMap::new(),
            failing: HashMap::new(),
            last_check_scan: None,
        })
    }

    /// One pass over what was stored since the last. Returns how many new events it wrote.
    pub fn pass(&mut self, conn: &Connection, config: &Config, now: i64) -> Result<usize> {
        if self.tokens_loaded.elapsed() > Duration::from_secs(600) {
            self.tokens = Tokens::load(conn)?;
            self.tokens_loaded = Instant::now();
        }
        let mut new = self.stored_transactions(conn, config, now)?;
        if self
            .last_check_scan
            .is_none_or(|t| t.elapsed() >= CHECK_SCAN_EVERY)
        {
            new += self.failed_checks(conn, config, now)?;
            self.last_check_scan = Some(Instant::now());
        }
        Ok(new)
    }

    fn stored_transactions(
        &mut self,
        conn: &Connection,
        config: &Config,
        now: i64,
    ) -> Result<usize> {
        let Some(start) = get_state(conn, START_SLOT)? else {
            // The first run sends only what is stored from now on, never the history.
            let slot: i64 =
                conn.query_row("SELECT COALESCE(MAX(slot), 0) FROM transactions", [], |r| {
                    r.get(0)
                })?;
            set_state(conn, START_SLOT, slot)?;
            set_state(conn, CURSOR, now)?;
            return Ok(0);
        };
        let cursor = get_state(conn, CURSOR)?.unwrap_or(now);
        let rows: Vec<(String, i64)> = conn
            .prepare_cached(
                "SELECT signature, indexed_at FROM transactions
                 WHERE indexed_at >= ?1 AND slot > ?2 AND succeeded = 1
                 ORDER BY indexed_at, signature",
            )?
            .query_map(params![cursor - OVERLAP, start], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut latest = cursor;
        let mut events = Vec::new();
        let mut looked = Vec::new();
        for (signature, indexed_at) in rows {
            latest = latest.max(indexed_at);
            if self.seen.get(&signature) == Some(&indexed_at) {
                continue;
            }
            self.transaction_events(conn, config, &signature, now, &mut events)?;
            looked.push((signature, indexed_at));
        }
        let written = write_events(conn, config, &events, now)?;
        // Marked only once written, so a failed write is retried on the next pass.
        self.seen.extend(looked);
        if latest > cursor {
            set_state(conn, CURSOR, latest)?;
        }
        self.seen.retain(|_, at| *at >= latest - OVERLAP);
        Ok(written)
    }

    fn transaction_events(
        &self,
        conn: &Connection,
        config: &Config,
        signature: &str,
        now: i64,
        events: &mut Vec<NewEvent>,
    ) -> Result<()> {
        let Some(tx) = api::transaction(conn, &self.tokens, signature)? else {
            return Ok(());
        };
        for d in &tx.donations {
            let id = event_id(
                EventType::DonationMade,
                &format!("{}:{}", d.signature, d.ordinal),
            );
            if event_exists(conn, &id)? {
                continue;
            }
            let data = DonationData {
                donation: d,
                url: config.link(&format!("/tx/{}", d.signature)),
            };
            events.push(NewEvent::new(
                id,
                EventType::DonationMade,
                &d.pool.config_id,
                &data,
                now,
            )?);
        }
        for touched in &tx.touched {
            let kind = match touched.action {
                "closed" => EventType::EpochClosed,
                "paid" => EventType::EpochSettled,
                _ => continue,
            };
            let p = &touched.pool;
            let id = event_id(kind, &format!("{}:{}:{}", p.config_id, p.mint, p.epoch));
            if event_exists(conn, &id)? {
                continue;
            }
            let Some(epoch) = api::epoch(conn, &self.tokens, &p.config_id, &p.mint, p.epoch)?
            else {
                continue;
            };
            let url = config.link(&format!("/epoch/{}/{}/{}", p.config_id, p.mint, p.epoch));
            let event = match (kind, &epoch.close, &epoch.payout) {
                (EventType::EpochClosed, Some(close), _) => NewEvent::new(
                    id,
                    kind,
                    &p.config_id,
                    &EpochClosedData {
                        pool: &epoch.pool,
                        signature: &tx.signature,
                        slot: tx.slot,
                        block_time: tx.block_time,
                        finalized: tx.finalized,
                        gross: &close.gross,
                        donation_count: epoch.donation_count,
                        url,
                    },
                    now,
                )?,
                (EventType::EpochSettled, _, Some(payout)) => NewEvent::new(
                    id,
                    kind,
                    &p.config_id,
                    &EpochSettledData {
                        pool: &epoch.pool,
                        signature: &tx.signature,
                        slot: tx.slot,
                        block_time: tx.block_time,
                        finalized: tx.finalized,
                        donation_count: epoch.donation_count,
                        donated: &epoch.donated,
                        swap: epoch.swap.as_ref(),
                        payout,
                        url,
                    },
                    now,
                )?,
                _ => continue,
            };
            events.push(event);
        }
        Ok(())
    }

    /// A failure is sent once a later check run finds it again, so an epoch caught between
    /// two transactions (a payout stored before its swap, say) never raises a false alarm.
    fn failed_checks(&mut self, conn: &Connection, config: &Config, now: i64) -> Result<usize> {
        type Row = (
            String,
            String,
            i64,
            String,
            Option<String>,
            Option<String>,
            String,
            i64,
        );
        let rows: Vec<Row> = conn
            .prepare_cached(
                "SELECT config_id, mint, epoch, check_id, expected, actual, detail, checked_at
                 FROM checks WHERE outcome = 'fail'",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut failing = HashMap::new();
        let mut events = Vec::new();
        for (config_id, mint, epoch, check_id, expected, actual, detail, checked_at) in rows {
            let id = event_id(
                EventType::CheckFailed,
                &format!("{config_id}:{mint}:{epoch}:{check_id}:{detail}"),
            );
            let first = self.failing.get(&id).copied().unwrap_or(checked_at);
            failing.insert(id.clone(), first);
            if checked_at <= first || event_exists(conn, &id)? {
                continue;
            }
            let name = Check::ALL
                .into_iter()
                .find(|c| c.id() == check_id)
                .map_or("unknown check", Check::title);
            let url = config.link(&format!("/epoch/{config_id}/{mint}/{epoch}"));
            let data = CheckFailedData {
                pool: Pool {
                    config_id: config_id.clone(),
                    mint,
                    epoch,
                },
                check: &check_id,
                name,
                expected: expected.as_deref(),
                actual: actual.as_deref(),
                detail: &detail,
                url,
            };
            events.push(NewEvent::new(
                id,
                EventType::CheckFailed,
                &config_id,
                &data,
                now,
            )?);
        }
        self.failing = failing;
        write_events(conn, config, &events, now)
    }
}

fn open(db: &Path) -> Result<Connection> {
    let conn = Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}

/// Runs the detector and the sender until the process exits.
pub async fn run(db: PathBuf, config: Arc<Config>) -> Result<()> {
    let notify = Arc::new(Notify::new());
    {
        let (db, config, notify) = (db.clone(), config.clone(), notify.clone());
        std::thread::Builder::new()
            .name("webhook-detector".into())
            .spawn(move || detect_forever(&db, &config, &notify))?;
    }
    send_forever(db, config, notify).await
}

fn detect_forever(db: &Path, config: &Config, notify: &Notify) {
    let mut detector = None;
    let mut conn = None;
    loop {
        let result = (|| -> Result<usize> {
            if conn.is_none() {
                conn = Some(open(db)?);
            }
            let c = conn.as_ref().expect("opened above");
            if detector.is_none() {
                detector = Some(Detector::new(c)?);
            }
            detector
                .as_mut()
                .expect("made above")
                .pass(c, config, unix_now())
        })();
        match result {
            Ok(0) => {}
            Ok(_) => notify.notify_one(),
            Err(error) => eprintln!("webhooks: finding events failed: {error:#}"),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

struct Due {
    event_id: String,
    endpoint: String,
    attempts: u32,
    kind: String,
    body: String,
}

enum Attempt {
    Delivered(u16),
    Failed {
        status: Option<u16>,
        error: String,
        /// The endpoint answered 410 Gone: stop retrying.
        gone: bool,
    },
}

async fn send_forever(db: PathBuf, config: Arc<Config>, notify: Arc<Notify>) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Relayscope-Webhooks/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let conn = Arc::new(Mutex::new(open(&db)?));
    loop {
        match send_due(&conn, &config, &http).await {
            // A full batch: more may be due right away.
            Ok(n) if n == BATCH => continue,
            Ok(_) => {}
            Err(error) => eprintln!("webhooks: sending failed: {error:#}"),
        }
        tokio::select! {
            _ = notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

const BATCH: usize = 64;

async fn send_due(
    conn: &Arc<Mutex<Connection>>,
    config: &Arc<Config>,
    http: &reqwest::Client,
) -> Result<usize> {
    let now = unix_now();
    let due = {
        let conn = conn.clone();
        let urls: Vec<String> = config.endpoints.iter().map(|e| e.url.clone()).collect();
        tokio::task::spawn_blocking(move || due(&conn.lock().expect("webhook db lock"), &urls, now))
            .await??
    };
    let count = due.len();
    let results: Vec<(Due, Attempt)> = futures::stream::iter(due)
        .map(|d| async move {
            let attempt = match config.endpoint(&d.endpoint) {
                Some(endpoint) => attempt(http, endpoint, &d.event_id, &d.body).await,
                None => Attempt::Failed {
                    status: None,
                    error: "endpoint removed from the config".into(),
                    gone: true,
                },
            };
            (d, attempt)
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        record(&conn.lock().expect("webhook db lock"), &results, unix_now())
    })
    .await??;
    Ok(count)
}

fn due(conn: &Connection, urls: &[String], now: i64) -> Result<Vec<Due>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; urls.len()].join(", ");
    let sql = format!(
        "SELECT d.event_id, d.endpoint, d.attempts, e.type, e.payload
         FROM webhook_deliveries d JOIN webhook_events e ON e.id = d.event_id
         WHERE d.state = 'pending' AND d.next_at <= ?1 AND d.endpoint IN ({marks})
         ORDER BY d.next_at, e.created_at LIMIT {BATCH}"
    );
    let values =
        std::iter::once(Value::Integer(now)).chain(urls.iter().map(|u| Value::Text(u.clone())));
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |r| {
        Ok(Due {
            event_id: r.get(0)?,
            endpoint: r.get(1)?,
            attempts: r.get(2)?,
            kind: r.get(3)?,
            body: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

async fn attempt(http: &reqwest::Client, endpoint: &Endpoint, id: &str, body: &str) -> Attempt {
    let timestamp = unix_now();
    let response = http
        .post(&endpoint.url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("webhook-id", id)
        .header("webhook-timestamp", timestamp.to_string())
        .header(
            "webhook-signature",
            sign(&endpoint.key, id, timestamp, body),
        )
        .body(body.to_string())
        .send()
        .await;
    match response {
        Ok(r) if r.status().is_success() => Attempt::Delivered(r.status().as_u16()),
        Ok(r) => Attempt::Failed {
            status: Some(r.status().as_u16()),
            error: format!("HTTP {}", r.status()),
            gone: r.status() == reqwest::StatusCode::GONE,
        },
        Err(error) => Attempt::Failed {
            status: None,
            error: error_chain(&error.without_url()),
            gone: false,
        },
    }
}

/// An error and its causes on one line. reqwest's own message alone is too vague to act on.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| u.port().map_or(h.to_string(), |p| format!("{h}:{p}")))
        })
        .unwrap_or_else(|| "the endpoint".into())
}

fn record(conn: &Connection, results: &[(Due, Attempt)], now: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (d, attempt) in results {
        let attempts = d.attempts + 1;
        let to = host(&d.endpoint);
        match attempt {
            Attempt::Delivered(status) => {
                tx.execute(
                    "UPDATE webhook_deliveries SET state = 'delivered', attempts = ?3, last_status = ?4,
                     last_error = NULL, delivered_at = ?5 WHERE event_id = ?1 AND endpoint = ?2",
                    params![d.event_id, d.endpoint, attempts, status, now],
                )?;
                eprintln!(
                    "webhooks: delivered {} {} to {to} (HTTP {status}, attempt {attempts})",
                    d.kind, d.event_id
                );
            }
            Attempt::Failed {
                status,
                error,
                gone,
            } => {
                let wait = if *gone { None } else { retry_after(attempts) };
                let (state, next_at) = match wait {
                    Some(wait) => ("pending", now + wait),
                    None => ("failed", now),
                };
                let error: String = error.chars().take(300).collect();
                tx.execute(
                    "UPDATE webhook_deliveries SET state = ?3, attempts = ?4, next_at = ?5,
                     last_status = ?6, last_error = ?7 WHERE event_id = ?1 AND endpoint = ?2",
                    params![
                        d.event_id, d.endpoint, state, attempts, next_at, status, error
                    ],
                )?;
                match wait {
                    Some(wait) => eprintln!(
                        "webhooks: {} {} to {to} failed ({error}); attempt {} in {wait} s",
                        d.kind,
                        d.event_id,
                        attempts + 1
                    ),
                    None => eprintln!(
                        "webhooks: {} {} to {to} failed ({error}); giving up after {attempts} attempts",
                        d.kind, d.event_id
                    ),
                }
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// `relayscope webhooks status`: deliveries by endpoint and state, and recent failures.
pub fn print_status(conn: &Connection) -> Result<()> {
    let events: i64 = conn.query_row("SELECT COUNT(*) FROM webhook_events", [], |r| r.get(0))?;
    if events == 0 {
        println!("No webhook events yet. Start `relayscope serve --webhooks <file>` to send them.");
        return Ok(());
    }
    let mut by_type =
        conn.prepare("SELECT type, COUNT(*) FROM webhook_events GROUP BY type ORDER BY type")?;
    println!("events");
    for row in by_type.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (kind, n) = row?;
        println!("  {kind:<14} {n:>8}");
    }
    let mut counts = conn.prepare(
        "SELECT endpoint,
                SUM(state = 'delivered'), SUM(state = 'pending'), SUM(state = 'failed')
         FROM webhook_deliveries GROUP BY endpoint ORDER BY endpoint",
    )?;
    println!("\ndeliveries      delivered  pending  failed");
    for row in counts.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (endpoint, delivered, pending, failed) = row?;
        println!("  {endpoint}\n                {delivered:>9}  {pending:>7}  {failed:>6}");
    }
    let mut failures = conn.prepare(
        "SELECT e.type, d.event_id, d.endpoint, d.state, d.attempts, d.last_error, d.next_at
         FROM webhook_deliveries d JOIN webhook_events e ON e.id = d.event_id
         WHERE d.state != 'delivered' AND d.attempts > 0
         ORDER BY d.next_at DESC LIMIT 10",
    )?;
    let mut printed = false;
    for row in failures.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })? {
        let (kind, id, endpoint, state, attempts, error, next_at) = row?;
        if !printed {
            println!("\nrecent failures");
            printed = true;
        }
        let when = if state == "failed" {
            "gave up".to_string()
        } else {
            format!("next attempt {}", rfc3339(next_at))
        };
        println!(
            "  {kind} {id} to {}: {} after {attempts} attempts, {when}",
            host(&endpoint),
            error.unwrap_or_default()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

    fn config(extra: &str) -> Config {
        Config::parse(
            &format!(
                "public_url = \"https://relayscope.example/\"\n[[endpoint]]\nurl = \"http://127.0.0.1:9/hook\"\nsecret = \"{SECRET}\"\n{extra}"
            ),
            |_| None,
        )
        .unwrap()
    }

    #[test]
    fn signs_like_the_standard_webhooks_spec() {
        // The example in the Standard Webhooks spec.
        let key = parse_secret(SECRET).unwrap();
        assert_eq!(
            sign(
                &key,
                "msg_p5jXN8AQM9LWM0D4loKWxJek",
                1_614_265_330,
                r#"{"test": 2432232314}"#
            ),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }

    #[test]
    fn new_secrets_parse() {
        let a = new_secret().unwrap();
        assert_eq!(parse_secret(&a).unwrap().len(), 32);
        assert_ne!(a, new_secret().unwrap());
        assert!(parse_secret("MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw").is_err());
        assert!(parse_secret("whsec_c2hvcnQ=").is_err());
    }

    #[test]
    fn retries_for_about_22_hours_then_gives_up() {
        assert_eq!(retry_after(0), None);
        assert_eq!(retry_after(1), Some(5));
        assert_eq!(retry_after(10), Some(28_800));
        assert_eq!(retry_after(11), None);
        let total: i64 = (1..=10).filter_map(retry_after).sum();
        assert!(total > 21 * 3_600 && total < 24 * 3_600, "{total}");
    }

    #[test]
    fn parses_the_config_file() {
        let c = config(
            "events = [\"donation.made\"]\nconfigs = [\"0x0000000000000000000000000000000000000000000000000000000000000001\"]",
        );
        assert_eq!(c.public_url.as_deref(), Some("https://relayscope.example"));
        let e = &c.endpoints[0];
        let one = "0x0000000000000000000000000000000000000000000000000000000000000001";
        assert!(e.wants(EventType::DonationMade, one));
        assert!(!e.wants(EventType::EpochSettled, one));
        assert!(!e.wants(EventType::DonationMade, "0x02"));

        let from_env = Config::parse(
            "[[endpoint]]\nurl = \"https://a.example\"\nsecret_env = \"HOOK\"",
            |name| (name == "HOOK").then(|| SECRET.to_string()),
        )
        .unwrap();
        assert_eq!(from_env.endpoints[0].events.len(), 4);

        for bad in [
            "[[endpoint]]\nurl = \"ftp://a\"\nsecret = \"whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw\"",
            "[[endpoint]]\nurl = \"https://a\"",
            "[[endpoint]]\nurl = \"https://a\"\nsecret = \"whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw\"\nevents = [\"donation.made2\"]",
            "[[endpoint]]\nurl = \"https://a\"\nsecret_env = \"MISSING\"",
            "[[endpoint]]\nurl = \"https://a\"\nsecret = \"whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw\"\ntypo = 1",
            "",
        ] {
            assert!(Config::parse(bad, |_| None).is_err(), "{bad}");
        }
    }

    fn deliveries(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT e.type, d.state FROM webhook_deliveries d JOIN webhook_events e ON e.id = d.event_id ORDER BY e.type")
            .unwrap()
            .query_map([], |r| Ok((r.get(0).unwrap(), r.get(1).unwrap())))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn first_run_skips_history() {
        let store = Store::with_fixtures();
        let conn = store.connection();
        let mut detector = Detector::new(conn).unwrap();
        assert_eq!(detector.pass(conn, &config(""), unix_now()).unwrap(), 0);
        assert_eq!(detector.pass(conn, &config(""), unix_now()).unwrap(), 0);
        assert!(get_state(conn, START_SLOT).unwrap().unwrap() > 0);
    }

    #[test]
    fn finds_each_event_once() {
        let store = Store::with_fixtures();
        let conn = store.connection();
        // Pretend webhooks were turned on before the fixtures were stored.
        set_state(conn, START_SLOT, 0).unwrap();
        set_state(conn, CURSOR, 0).unwrap();
        let config = config("");
        let mut detector = Detector::new(conn).unwrap();
        let now = unix_now();
        let first = detector.pass(conn, &config, now).unwrap();
        assert!(first > 0);
        let kinds: HashSet<String> = deliveries(conn).into_iter().map(|(k, _)| k).collect();
        for kind in ["donation.made", "epoch.closed", "epoch.settled"] {
            assert!(kinds.contains(kind), "no {kind} in {kinds:?}");
        }

        // Rewriting every transaction (a reindex, or the finalizer) sends nothing again.
        let mut fresh = Detector::new(conn).unwrap();
        conn.execute("UPDATE transactions SET indexed_at = indexed_at + 1", [])
            .unwrap();
        assert_eq!(fresh.pass(conn, &config, now + 1).unwrap(), 0);

        // Payloads carry the envelope and a link.
        let body: String = conn
            .query_row(
                "SELECT payload FROM webhook_events WHERE type = 'donation.made' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["type"], "donation.made");
        assert!(v["timestamp"].as_str().unwrap().ends_with('Z'));
        assert!(
            v["data"]["url"]
                .as_str()
                .unwrap()
                .starts_with("https://relayscope.example/tx/")
        );
        assert!(v["data"]["amount"]["raw"].is_string());
    }

    #[test]
    fn sends_a_check_failure_once_a_second_run_confirms_it() {
        let store = Store::with_fixtures();
        let conn = store.connection();
        let config = config("");
        let mut detector = Detector::new(conn).unwrap();
        detector.pass(conn, &config, unix_now()).unwrap();
        conn.execute(
            "INSERT INTO checks (config_id, mint, epoch, check_id, outcome, expected, actual, detail, checked_at)
             VALUES ('0x01', 'mint', 7, 'C3', 'fail', '10', '9', 'lines add up to 9', 100)",
            [],
        )
        .unwrap();
        detector.last_check_scan = None;
        assert_eq!(
            detector.failed_checks(conn, &config, 200).unwrap(),
            0,
            "one run is not enough"
        );
        assert_eq!(
            detector.failed_checks(conn, &config, 200).unwrap(),
            0,
            "the same run again"
        );
        conn.execute(
            "UPDATE checks SET checked_at = 700 WHERE check_id = 'C3'",
            [],
        )
        .unwrap();
        assert_eq!(detector.failed_checks(conn, &config, 800).unwrap(), 1);
        assert_eq!(
            detector.failed_checks(conn, &config, 900).unwrap(),
            0,
            "sent once"
        );
        let body: String = conn
            .query_row(
                "SELECT payload FROM webhook_events WHERE type = 'check.failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["data"]["check"], "C3");
        assert_eq!(v["data"]["name"], "settlement total");
        assert_eq!(
            v["data"]["url"],
            "https://relayscope.example/epoch/0x01/mint/7"
        );
    }

    #[tokio::test]
    async fn delivers_signed_requests_and_retries_failures() {
        use axum::http::{HeaderMap, StatusCode};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A receiver that fails the first request and accepts the rest, checking signatures.
        let calls = Arc::new(AtomicUsize::new(0));
        let verified = Arc::new(AtomicUsize::new(0));
        let key = parse_secret(SECRET).unwrap();
        let app = {
            let (calls, verified, key) = (calls.clone(), verified.clone(), key.clone());
            axum::Router::new().route(
                "/hook",
                axum::routing::post(move |headers: HeaderMap, body: String| {
                    let (calls, verified, key) = (calls.clone(), verified.clone(), key.clone());
                    async move {
                        let h = |n: &str| headers.get(n).unwrap().to_str().unwrap().to_string();
                        let ts: i64 = h("webhook-timestamp").parse().unwrap();
                        if sign(&key, &h("webhook-id"), ts, &body) == h("webhook-signature") {
                            verified.fetch_add(1, Ordering::SeqCst);
                        }
                        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            StatusCode::INTERNAL_SERVER_ERROR
                        } else {
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let config = Arc::new(
            Config::parse(
                &format!(
                    "[[endpoint]]\nurl = \"http://127.0.0.1:{port}/hook\"\nsecret = \"{SECRET}\""
                ),
                |_| None,
            )
            .unwrap(),
        );
        let store = Store::open_in_memory().unwrap();
        let now = unix_now();
        let event = NewEvent::new(
            event_id(EventType::DonationMade, "sig:0"),
            EventType::DonationMade,
            "0x01",
            &serde_json::json!({ "hello": "world" }),
            now,
        )
        .unwrap();
        write_events(store.connection(), &config, &[event], now).unwrap();
        let conn = Arc::new(Mutex::new(store.into_connection()));
        let http = reqwest::Client::new();

        assert_eq!(send_due(&conn, &config, &http).await.unwrap(), 1);
        let (state, attempts, next_at): (String, u32, i64) = conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT state, attempts, next_at FROM webhook_deliveries",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), attempts), ("pending", 1));
        assert!(next_at >= now + 5);

        // Not due yet, so nothing is sent.
        assert_eq!(send_due(&conn, &config, &http).await.unwrap(), 0);
        conn.lock()
            .unwrap()
            .execute("UPDATE webhook_deliveries SET next_at = 0", [])
            .unwrap();
        assert_eq!(send_due(&conn, &config, &http).await.unwrap(), 1);
        let (state, attempts): (String, u32) = conn
            .lock()
            .unwrap()
            .query_row("SELECT state, attempts FROM webhook_deliveries", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((state.as_str(), attempts), ("delivered", 2));
        assert_eq!(verified.load(Ordering::SeqCst), 2);
    }
}
