//! `relayscope live`: follows the program in real time.
//!
//! Two sources can announce new transactions:
//!
//! - **WebSocket** (the default): a `logsSubscribe` on any Solana RPC WebSocket, such as a free
//!   Helius key. It announces each transaction's signature at `confirmed` commitment, and the
//!   loop fetches the transaction through the same rate-limited RPC client as the backfill.
//! - **gapless**: Solami's Yellowstone gRPC stream through gapless, which delivers whole
//!   transactions and replays any gap itself. It needs a Solami plan with gRPC access.
//!
//! Either way, every transaction is decoded and stored by the backfill's code and marked
//! provisional. A finalizer re-fetches provisional transactions once their slot is finalized.
//! A repair pass lists the program's newest signatures from RPC and fetches anything the source
//! did not deliver, so a missed transaction is caught and filled in; every repair is logged.
//! After each repair, every reconciliation check runs again.
//!
//! With `--kill-test`, the source is cut on purpose and held down. After it recovers, every
//! signature RPC lists for the window is checked against the store, and a report is written.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use futures::{SinkExt, StreamExt};
use gapless::solami::geyser::SubscribeUpdateTransactionInfo;
use gapless::solami::solana::storage::confirmed_block::TokenBalance;
use gapless::{CommitmentLevel, Event, Gapless, Origin as StreamOrigin, State};
use relayscope_core::{Event as RelayEvent, PROGRAM_ID};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::backfill;
use crate::rpc::Rpc;
use crate::store::{Origin, SignatureRow, Store, decode_fetched};
use crate::verify;

/// Where new transactions come from.
pub enum Source {
    /// A Solana RPC WebSocket URL. The URL may hold an API key and is never printed.
    WebSocket { url: String },
    /// Solami's gRPC stream through gapless.
    Gapless {
        api_key: String,
        grpc_url: Option<String>,
    },
}

pub struct LiveOptions {
    pub source: Source,
    pub finalize_every: Duration,
    pub repair_every: Duration,
    pub kill_test: Option<KillTestOptions>,
}

pub struct KillTestOptions {
    /// The least time the source runs before it is cut.
    pub warmup: Duration,
    /// Transactions the source must deliver live before the cut.
    pub min_live: usize,
    /// The least time the source stays down.
    pub offline: Duration,
    /// Transactions RPC must list during the outage before the WebSocket comes back, so the
    /// test always has something to recover. gapless stays down for exactly `offline`.
    pub min_gap: usize,
    /// The WebSocket comes back after this long even if fewer transactions landed.
    pub max_offline: Duration,
    /// How long to keep running after recovery, so the window finalizes.
    pub settle: Duration,
    pub report_dir: PathBuf,
}

/// Builds a JSON-RPC `getTransaction` result from a streamed transaction, so it goes through
/// exactly the decoder and store the backfill uses. The block time is unknown until the
/// finalizer re-fetches it.
pub fn rpc_json(slot: u64, info: &SubscribeUpdateTransactionInfo) -> Result<Value> {
    let b58 = |bytes: &[u8]| bs58::encode(bytes).into_string();
    let tx = info
        .transaction
        .as_ref()
        .context("streamed update without a transaction")?;
    let meta = info
        .meta
        .as_ref()
        .context("streamed update without status meta")?;
    let message = tx
        .message
        .as_ref()
        .context("streamed transaction without a message")?;

    let instructions: Vec<Value> = message
        .instructions
        .iter()
        .map(|ix| json!({ "programIdIndex": ix.program_id_index, "accounts": ix.accounts, "data": b58(&ix.data), "stackHeight": null }))
        .collect();
    let inner: Vec<Value> = meta
        .inner_instructions
        .iter()
        .map(|group| {
            let list: Vec<Value> = group
                .instructions
                .iter()
                .map(|ix| json!({ "programIdIndex": ix.program_id_index, "accounts": ix.accounts, "data": b58(&ix.data), "stackHeight": ix.stack_height }))
                .collect();
            json!({ "index": group.index, "instructions": list })
        })
        .collect();
    let balances = |list: &[TokenBalance]| -> Vec<Value> {
        list.iter()
            .map(|b| {
                let (amount, decimals) = b
                    .ui_token_amount
                    .as_ref()
                    .map_or(("0".to_string(), 0), |u| (u.amount.clone(), u.decimals));
                json!({
                    "accountIndex": b.account_index,
                    "mint": b.mint,
                    "owner": if b.owner.is_empty() { Value::Null } else { json!(b.owner) },
                    "uiTokenAmount": { "amount": amount, "decimals": decimals },
                })
            })
            .collect()
    };
    let err = meta
        .err
        .as_ref()
        .map(|e| json!({ "streamed": hex::encode(&e.err) }));

    Ok(json!({
        "slot": slot,
        "blockTime": null,
        "transaction": {
            "signatures": tx.signatures.iter().map(|s| b58(s)).collect::<Vec<_>>(),
            "message": {
                "accountKeys": message.account_keys.iter().map(|k| b58(k)).collect::<Vec<_>>(),
                "instructions": instructions,
            },
        },
        "meta": {
            "err": err,
            "innerInstructions": inner,
            "loadedAddresses": {
                "writable": meta.loaded_writable_addresses.iter().map(|k| b58(k)).collect::<Vec<_>>(),
                "readonly": meta.loaded_readonly_addresses.iter().map(|k| b58(k)).collect::<Vec<_>>(),
            },
            "preTokenBalances": balances(&meta.pre_token_balances),
            "postTokenBalances": balances(&meta.post_token_balances),
            "logMessages": meta.log_messages,
        },
    }))
}

fn stamp() -> String {
    chrono::DateTime::from_timestamp(unix_now(), 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

macro_rules! log {
    ($($arg:tt)*) => { eprintln!("{} {}", stamp(), format!($($arg)*)) };
}

/// What a source reports to the live loop.
enum Incoming {
    /// A new transaction. `raw` is the whole transaction when the source carried it (gapless);
    /// otherwise the loop fetches it.
    Transaction {
        signature: String,
        slot: u64,
        failed: bool,
        raw: Option<Value>,
        origin: Origin,
    },
    Up(String),
    Down(String),
    /// The source closed an outage itself (gapless replayed the gap).
    Recovered(String),
    Note(String),
    Ended(String),
}

/// A request to the WebSocket task.
enum WsCommand {
    /// Close the connection and stay down for up to this long.
    Cut(Duration),
    /// Reconnect now.
    Restore,
}

/// Lets the loop cut its source on purpose, for the kill test.
enum Control {
    Gapless(gapless::Control),
    WebSocket(mpsc::UnboundedSender<WsCommand>),
}

impl Control {
    fn cut(&self, offline: Duration) {
        match self {
            Control::Gapless(c) => {
                c.hold(Some(offline));
                c.cut();
            }
            Control::WebSocket(tx) => {
                let _ = tx.send(WsCommand::Cut(offline));
            }
        }
    }

    /// Ends a WebSocket outage early. gapless keeps its fixed hold.
    fn restore(&self) {
        if let Control::WebSocket(tx) = self {
            let _ = tx.send(WsCommand::Restore);
        }
    }

    fn stop(&self) {
        if let Control::Gapless(c) = self {
            c.stop();
        }
    }
}

fn start_gapless(
    api_key: &str,
    grpc_url: Option<String>,
) -> Result<(mpsc::UnboundedReceiver<Incoming>, Control)> {
    let mut builder = Gapless::builder(api_key)
        .program(PROGRAM_ID.to_string())
        .include_failed(true)
        .commitment(CommitmentLevel::Confirmed);
    if let Some(url) = grpc_url {
        builder = builder.grpc_url(url);
    }
    let (mut events, control) = builder.build().context("starting gapless")?.start();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let message = match event {
                Event::Transaction(t) => {
                    let signature = t.signature.to_string();
                    let origin = match t.origin {
                        StreamOrigin::Live => Origin::Live,
                        StreamOrigin::Replay { .. } => Origin::Replay,
                    };
                    match rpc_json(t.slot, &t.info) {
                        Ok(raw) => Incoming::Transaction {
                            failed: !raw["meta"]["err"].is_null(),
                            signature,
                            slot: t.slot,
                            raw: Some(raw),
                            origin,
                        },
                        Err(error) => Incoming::Note(format!(
                            "could not read streamed {signature}: {error:#}"
                        )),
                    }
                }
                Event::State(State::Live) => Incoming::Up("gapless is live".into()),
                Event::State(state) => Incoming::Down(format!("gapless: {state:?}")),
                Event::Disconnected(d) => Incoming::Note(format!(
                    "gapless disconnected (incident {}, step {}): {:?} {}",
                    d.incident, d.step, d.reason, d.detail
                )),
                Event::Recovered(i) => Incoming::Recovered(format!(
                    "gapless recovered from incident {} after {:?}: gap {:?}, replayed {}, duplicates dropped {}, unrecoverable {:?}",
                    i.id,
                    i.duration(),
                    i.gap,
                    i.replayed,
                    i.duplicates,
                    i.unrecoverable
                )),
                Event::HandoffPatched {
                    incident,
                    slots,
                    recovered,
                    error,
                } => Incoming::Note(format!(
                    "gapless handoff patch for incident {incident}: re-read {slots:?}, recovered {recovered}, error {error:?}"
                )),
                _ => continue,
            };
            if tx.send(message).is_err() {
                return;
            }
        }
        let _ = tx.send(Incoming::Ended("the gapless stream ended".into()));
    });
    Ok((rx, Control::Gapless(control)))
}

fn start_websocket(url: String) -> (mpsc::UnboundedReceiver<Incoming>, Control) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (cut_tx, mut cut_rx) = mpsc::unbounded_channel::<WsCommand>();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        while !tx.is_closed() {
            match websocket_session(&url, &tx, &mut cut_rx).await {
                Ok(Some(offline)) => {
                    let _ = tx.send(Incoming::Down(format!(
                        "websocket cut on purpose, staying down for up to {offline:?}"
                    )));
                    let wake = tokio::time::sleep(offline);
                    tokio::pin!(wake);
                    loop {
                        tokio::select! {
                            _ = &mut wake => break,
                            command = cut_rx.recv() => match command {
                                Some(WsCommand::Restore) | None => break,
                                Some(WsCommand::Cut(_)) => {}
                            },
                        }
                    }
                    backoff = Duration::from_secs(1);
                    continue;
                }
                Ok(None) => {
                    let _ = tx.send(Incoming::Down("websocket closed by the server".into()));
                }
                Err(error) => {
                    let _ = tx.send(Incoming::Down(format!("websocket error: {error:#}")));
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    (rx, Control::WebSocket(cut_tx))
}

/// One WebSocket connection: subscribe, forward notifications, ping, until it closes or the
/// loop asks for a cut. Returns `Some(offline)` for a cut.
async fn websocket_session(
    url: &str,
    tx: &mpsc::UnboundedSender<Incoming>,
    cut_rx: &mut mpsc::UnboundedReceiver<WsCommand>,
) -> Result<Option<Duration>> {
    // The URL can carry an API key, so errors are reported without it.
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| anyhow!("could not connect: {}", short_ws_error(&e)))?;
    let subscribe = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "logsSubscribe",
        "params": [{ "mentions": [PROGRAM_ID.to_string()] }, { "commitment": "confirmed" }],
    });
    ws.send(Message::Text(subscribe.to_string().into())).await?;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.tick().await;
    loop {
        tokio::select! {
            message = ws.next() => {
                let Some(message) = message else { return Ok(None) };
                match message.map_err(|e| anyhow!("{}", short_ws_error(&e)))? {
                    Message::Text(text) => {
                        let v: Value = serde_json::from_str(&text)?;
                        if v.get("id").is_some() {
                            if let Some(error) = v.get("error") {
                                bail!("logsSubscribe was rejected: {error}");
                            }
                            let _ = tx.send(Incoming::Up(format!("websocket subscribed (subscription {})", v["result"])));
                            continue;
                        }
                        if v["method"] == "logsNotification" {
                            let result = &v["params"]["result"];
                            let (Some(signature), Some(slot)) = (result["value"]["signature"].as_str(), result["context"]["slot"].as_u64()) else {
                                continue;
                            };
                            let _ = tx.send(Incoming::Transaction {
                                signature: signature.to_owned(),
                                slot,
                                failed: !result["value"]["err"].is_null(),
                                raw: None,
                                origin: Origin::Live,
                            });
                        }
                    }
                    Message::Close(_) => return Ok(None),
                    _ => {}
                }
            }
            _ = ping.tick() => ws.send(Message::Ping(Vec::new().into())).await?,
            command = cut_rx.recv() => {
                if let Some(WsCommand::Cut(offline)) = command {
                    let _ = ws.close(None).await;
                    return Ok(Some(offline));
                }
            }
        }
    }
}

/// A WebSocket error without any URL in it.
fn short_ws_error(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as E;
    match error {
        E::Http(response) => format!("HTTP {}", response.status()),
        E::Io(e) => format!("I/O error: {}", e.kind()),
        E::Tls(_) => "TLS error".into(),
        E::Url(_) => "invalid WebSocket URL".into(),
        E::ConnectionClosed | E::AlreadyClosed => "connection closed".into(),
        E::Protocol(p) => format!("protocol error: {p}"),
        _ => "other WebSocket error".to_string(),
    }
}

/// Fetches a transaction at `confirmed`, retrying briefly because a fresh notification can
/// arrive before the RPC node serves the transaction.
async fn fetch_confirmed(rpc: &Rpc, signature: &str) -> Result<Value> {
    let mut last = anyhow!("not attempted");
    for _ in 0..8 {
        match rpc.transaction_at(signature, "confirmed").await {
            Ok(raw) => return Ok(raw),
            Err(error) => last = error,
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last)
}

/// What the kill test is waiting for.
enum KillStage {
    WaitingForUp,
    WarmingUp {
        since: Instant,
    },
    Down {
        cut_slot: u64,
        cut_at: Instant,
        last_probe: Instant,
    },
    Settling {
        outage: Outage,
        until: Instant,
    },
}

/// An outage the kill test caused.
struct Outage {
    cut_slot: u64,
    recovered_slot: u64,
    seconds: f64,
    recovered_by: String,
}

pub async fn run(rpc: &Rpc, store: &mut Store, options: LiveOptions) -> Result<bool> {
    let (mut incoming, control, name) = match options.source {
        Source::WebSocket { url } => {
            let (rx, control) = start_websocket(url);
            (rx, control, "websocket")
        }
        Source::Gapless { api_key, grpc_url } => {
            let (rx, control) = start_gapless(&api_key, grpc_url)?;
            (rx, control, "gapless")
        }
    };
    log!("live: following {PROGRAM_ID} at confirmed commitment through {name}");

    // Catch up with anything that landed since the last run, before relying on the source.
    repair(rpc, store, "startup").await;

    let mut finalize_tick = tokio::time::interval(options.finalize_every);
    let mut repair_tick = tokio::time::interval(options.repair_every);
    finalize_tick.tick().await;
    repair_tick.tick().await;

    // Every signature the source announced, with its slot and when it arrived.
    let mut announced: HashMap<String, (u64, SystemTime)> = HashMap::new();
    let mut down_since: Option<Instant> = Some(Instant::now());
    let mut kill = options.kill_test.as_ref().map(|_| KillStage::WaitingForUp);

    loop {
        tokio::select! {
            message = incoming.recv() => match message {
                None => {
                    log!("live: the {name} source ended");
                    return Ok(false);
                }
                Some(Incoming::Ended(why)) => {
                    log!("live: {why}");
                    return Ok(false);
                }
                Some(Incoming::Transaction { signature, slot, failed, raw, origin }) => {
                    let first = announced.insert(signature.clone(), (slot, SystemTime::now())).is_none();
                    if !first && raw.is_none() {
                        continue;
                    }
                    let raw = match raw {
                        Some(raw) => raw,
                        None => match fetch_confirmed(rpc, &signature).await {
                            Ok(raw) => raw,
                            Err(error) => {
                                log!("live: could not fetch {signature} yet, the next repair will: {error:#}");
                                continue;
                            }
                        },
                    };
                    let item = decode_fetched(signature.clone(), raw);
                    if let Ok(decoded) = &item.decoded {
                        for e in &decoded.events {
                            if let RelayEvent::DonationMadeV1Event(d) = &e.event {
                                log!("donation: {} base units of {} to config {}, slot {slot}", d.gross_amount, d.mint, d.config_id);
                            }
                        }
                    }
                    let row = SignatureRow { signature, slot, block_time: None, failed };
                    if let Err(error) = store.write_streamed(&row, item, origin) {
                        log!("live: could not store {}: {error:#}", row.signature);
                    }
                }
                Some(Incoming::Up(what)) => {
                    log!("source: {what}");
                    down_since = None;
                    match &kill {
                        Some(KillStage::WaitingForUp) => kill = Some(KillStage::WarmingUp { since: Instant::now() }),
                        Some(KillStage::Down { cut_slot, cut_at, .. }) if name == "websocket" => {
                            // The WebSocket does not replay; the repair pass fills the gap.
                            let recovered_slot = rpc.current_slot().await.unwrap_or(*cut_slot);
                            let seconds = cut_at.elapsed().as_secs_f64();
                            repair(rpc, store, "recovery after the outage").await;
                            let settle = options.kill_test.as_ref().map_or(Duration::ZERO, |k| k.settle);
                            kill = Some(KillStage::Settling {
                                outage: Outage { cut_slot: *cut_slot, recovered_slot, seconds, recovered_by: "the repair pass, which listed the gap from RPC and fetched what was missing".into() },
                                until: Instant::now() + settle,
                            });
                        }
                        _ => {}
                    }
                }
                Some(Incoming::Down(why)) => {
                    log!("source: {why}");
                    down_since.get_or_insert_with(Instant::now);
                }
                Some(Incoming::Recovered(what)) => {
                    log!("source: {what}");
                    if let Some(KillStage::Down { cut_slot, cut_at, .. }) = &kill {
                        let recovered_slot = rpc.current_slot().await.unwrap_or(*cut_slot);
                        let settle = options.kill_test.as_ref().map_or(Duration::ZERO, |k| k.settle);
                        kill = Some(KillStage::Settling {
                            outage: Outage { cut_slot: *cut_slot, recovered_slot, seconds: cut_at.elapsed().as_secs_f64(), recovered_by: format!("gapless replay ({what})") },
                            until: Instant::now() + settle,
                        });
                    }
                }
                Some(Incoming::Note(note)) => log!("source: {note}"),
            },
            _ = finalize_tick.tick() => finalize(rpc, store).await,
            _ = repair_tick.tick() => {
                repair(rpc, store, "scheduled").await;
                check(store);
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                // If the source is down for over a minute outside a kill test, poll RPC instead.
                if kill.is_none()
                    && let Some(since) = down_since
                    && since.elapsed() > Duration::from_secs(60)
                {
                    repair(rpc, store, "source down, polling RPC").await;
                    down_since = Some(Instant::now());
                }
                let Some(test) = &options.kill_test else { continue };
                match &kill {
                    Some(KillStage::WarmingUp { since })
                        if since.elapsed() >= test.warmup
                            && (announced.len() >= test.min_live || since.elapsed() >= Duration::from_secs(7_200)) =>
                    {
                        let cut_slot = rpc.current_slot().await?;
                        let hold = if name == "websocket" { test.max_offline } else { test.offline };
                        log!(
                            "kill test: {} transactions arrived live; cutting the {name} source at slot {cut_slot}",
                            announced.len()
                        );
                        control.cut(hold);
                        kill = Some(KillStage::Down { cut_slot, cut_at: Instant::now(), last_probe: Instant::now() });
                    }
                    Some(KillStage::Down { cut_slot, cut_at, last_probe })
                        if name == "websocket" && last_probe.elapsed() >= Duration::from_secs(30) =>
                    {
                        let (cut_slot, cut_at) = (*cut_slot, *cut_at);
                        let landed = rpc_count_after(rpc, cut_slot).await.unwrap_or(0);
                        if cut_at.elapsed() >= test.offline && landed >= test.min_gap {
                            log!("kill test: {landed} transactions landed during the outage; bringing the websocket back");
                            control.restore();
                        } else {
                            log!("kill test: down for {:.0} seconds, {landed} transactions landed so far", cut_at.elapsed().as_secs_f64());
                        }
                        kill = Some(KillStage::Down { cut_slot, cut_at, last_probe: Instant::now() });
                    }
                    Some(KillStage::Down { cut_at, .. })
                        if cut_at.elapsed() > test.max_offline.max(test.offline) + Duration::from_secs(600) =>
                    {
                        log!("kill test: no recovery within 10 minutes of the longest planned outage");
                        return Ok(false);
                    }
                    Some(KillStage::Settling { until, .. }) if Instant::now() >= *until => {
                        let Some(KillStage::Settling { outage, .. }) = kill.take() else { unreachable!() };
                        control.stop();
                        finalize(rpc, store).await;
                        return kill_test_report(rpc, store, &announced, &outage, test, name).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Re-fetches provisional transactions whose slot is finalized, and rewrites them.
async fn finalize(rpc: &Rpc, store: &mut Store) {
    // Skip the RPC call when nothing is waiting.
    if !store.has_provisional().unwrap_or(true) {
        return;
    }
    let finalized = match rpc.finalized_slot().await {
        Ok(slot) => slot,
        Err(error) => return log!("finalize: could not read the finalized slot: {error:#}"),
    };
    let signatures = match store.provisional_up_to(finalized) {
        Ok(s) if s.is_empty() => return,
        Ok(s) => s,
        Err(error) => return log!("finalize: {error:#}"),
    };
    let mut items = Vec::new();
    for signature in signatures {
        match rpc.transaction(&signature).await {
            Ok(raw) => items.push(decode_fetched(signature, raw)),
            Err(error) => log!("finalize: {signature} not available at finalized yet: {error:#}"),
        }
    }
    let n = items.len();
    match store.write_fetched(&items, Origin::Live) {
        Ok(()) => log!("finalize: {n} transactions finalized"),
        Err(error) => log!("finalize: could not store: {error:#}"),
    }
}

/// Lists the newest signatures from RPC and fetches any the store does not have yet.
async fn repair(rpc: &Rpc, store: &mut Store, why: &str) {
    match backfill::list(rpc, store).await {
        Ok(listed) if listed.added > 0 => {
            log!(
                "repair ({why}): RPC listed {} signatures not yet stored; fetching them",
                listed.added
            );
            let options = backfill::FetchOptions {
                concurrency: 4,
                batch: 50,
                max: None,
            };
            match backfill::fetch(rpc, store, &options).await {
                Ok(report) => log!(
                    "repair: stored {}, {} decode failures, {} not fetched",
                    report.stored,
                    report.decode_failures,
                    report.fetch_failures
                ),
                Err(error) => log!("repair: fetch failed: {error:#}"),
            }
        }
        Ok(_) => log!("repair ({why}): nothing missing"),
        Err(error) => log!("repair ({why}): listing failed: {error:#}"),
    }
}

/// Runs every reconciliation check and logs the outcome.
fn check(store: &mut Store) {
    match verify::quick(store) {
        Ok(s) => log!(
            "checks: {} epochs, {} failures, {} epochs open or unpaid",
            s.epochs,
            s.failures,
            s.pending
        ),
        Err(error) => log!("checks: {error:#}"),
    }
}

/// Compares what RPC lists for the program around the outage with what the store holds,
/// writes a Markdown report and returns whether nothing was missing.
async fn kill_test_report(
    rpc: &Rpc,
    store: &Store,
    announced: &HashMap<String, (u64, SystemTime)>,
    outage: &Outage,
    test: &KillTestOptions,
    source: &str,
) -> Result<bool> {
    let finalized = rpc.finalized_slot().await?;
    let start = outage.cut_slot.saturating_sub(150);
    let end = (outage.recovered_slot + 150).min(finalized);
    let expected = rpc_window(rpc, start, end).await?;
    let in_gap = |slot: u64| slot >= outage.cut_slot && slot <= outage.recovered_slot;

    struct Row {
        signature: String,
        slot: u64,
        stored: bool,
        via_source: bool,
    }
    let mut rows = Vec::new();
    for (signature, slot) in &expected {
        rows.push(Row {
            stored: store.is_stored(signature)?,
            via_source: announced.contains_key(signature),
            signature: signature.clone(),
            slot: *slot,
        });
    }
    let listed: HashSet<&String> = expected.iter().map(|(s, _)| s).collect();
    let extra: Vec<(&String, u64)> = announced
        .iter()
        .filter(|(s, (slot, _))| *slot >= start && *slot <= end && !listed.contains(s))
        .map(|(s, (slot, _))| (s, *slot))
        .collect();
    let missing: Vec<&Row> = rows.iter().filter(|r| !r.stored).collect();
    let gap_rows: Vec<&Row> = rows.iter().filter(|r| in_gap(r.slot)).collect();
    let by_source = rows.iter().filter(|r| r.stored && r.via_source).count();
    let by_repair = rows.iter().filter(|r| r.stored && !r.via_source).count();

    // How long after its block a transaction reached the store through the source.
    let mut delays: Vec<i64> = Vec::new();
    for (signature, (_, at)) in announced {
        if let Some(block_time) = store.block_time(signature)? {
            let at = at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            delays.push(at - block_time);
        }
    }
    delays.sort_unstable();
    let delay = if delays.is_empty() {
        "no transactions with a known block time".to_string()
    } else {
        format!(
            "median {} seconds, longest {} seconds, over {} transactions",
            delays[delays.len() / 2],
            delays[delays.len() - 1],
            delays.len()
        )
    };
    let passed = missing.is_empty();

    let mut r = String::new();
    r.push_str(&format!("# Kill test, {} UTC\n\n", stamp()));
    r.push_str(&format!(
        "Relayscope followed the Donation Relay program through the {source} source at `confirmed` commitment. The source was cut at slot {} and held down for {:.0} seconds (at least {}, and until transactions had landed during the outage). After it came back, every signature RPC lists for the program between slots {start} and {end}, at `finalized` commitment, was looked up in the store.\n\n",
        outage.cut_slot,
        outage.seconds,
        test.offline.as_secs()
    ));
    r.push_str("| | |\n| --- | --- |\n");
    r.push_str(&format!(
        "| Result | {} |\n",
        if passed {
            "pass: nothing missing"
        } else {
            "FAIL: transactions missing"
        }
    ));
    r.push_str(&format!(
        "| Outage | slots {} to {}, {:.0} seconds |\n",
        outage.cut_slot, outage.recovered_slot, outage.seconds
    ));
    r.push_str(&format!("| Gap filled by | {} |\n", outage.recovered_by));
    r.push_str(&format!("| Window checked | slots {start} to {end} |\n"));
    r.push_str(&format!(
        "| Listed by RPC | {} signatures, {} of them during the outage |\n",
        rows.len(),
        gap_rows.len()
    ));
    r.push_str(&format!(
        "| Stored | {} through the {source} source, {} by the repair pass |\n",
        by_source, by_repair
    ));
    r.push_str(&format!("| Missing from the store | {} |\n", missing.len()));
    r.push_str(&format!(
        "| Announced but not listed by RPC | {} |\n",
        extra.len()
    ));
    r.push_str(&format!(
        "| Delay from block to store, through the source | {delay} |\n\n"
    ));
    r.push_str("## During the outage\n\n");
    if gap_rows.is_empty() {
        r.push_str("No transactions landed while the source was down.\n\n");
    }
    for row in &gap_rows {
        r.push_str(&format!(
            "- slot {}: `{}`, {}\n",
            row.slot,
            row.signature,
            match (row.stored, row.via_source) {
                (true, true) => "stored, announced by the source",
                (true, false) => "stored by the repair pass",
                (false, _) => "MISSING",
            }
        ));
    }
    if !missing.is_empty() {
        r.push_str("\n## Missing\n\n");
        for row in &missing {
            r.push_str(&format!("- slot {}: `{}`\n", row.slot, row.signature));
        }
    }
    if !extra.is_empty() {
        r.push_str("\n## Announced but not listed by RPC\n\n");
        for (signature, slot) in &extra {
            r.push_str(&format!("- slot {slot}: `{signature}`\n"));
        }
    }

    std::fs::create_dir_all(&test.report_dir)?;
    let path = test
        .report_dir
        .join(format!("kill-test-{}.md", stamp().replace([' ', ':'], "-")));
    std::fs::write(&path, &r)?;
    print!("{r}");
    log!("kill test: report written to {}", path.display());
    Ok(passed)
}

/// How many transactions RPC lists for the program after `slot`.
async fn rpc_count_after(rpc: &Rpc, slot: u64) -> Result<usize> {
    let program = PROGRAM_ID.to_string();
    let mut count = 0;
    let mut before: Option<String> = None;
    loop {
        let page = rpc
            .signatures_for_address(&program, before.as_deref(), None, 1_000)
            .await?;
        let Some(last) = page.last() else { break };
        before = Some(last.signature.clone());
        let oldest = last.slot;
        count += page.iter().filter(|r| r.slot > slot).count();
        if oldest <= slot {
            break;
        }
    }
    Ok(count)
}

/// Every signature RPC lists for the program in `[start, end]`, newest first.
async fn rpc_window(rpc: &Rpc, start: u64, end: u64) -> Result<Vec<(String, u64)>> {
    let program = PROGRAM_ID.to_string();
    let mut out = Vec::new();
    let mut before: Option<String> = None;
    loop {
        let page = rpc
            .signatures_for_address(&program, before.as_deref(), None, 1_000)
            .await?;
        let Some(last) = page.last() else { break };
        before = Some(last.signature.clone());
        let oldest = last.slot;
        out.extend(
            page.into_iter()
                .filter(|r| r.slot >= start && r.slot <= end)
                .map(|r| (r.signature, r.slot)),
        );
        if oldest < start {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gapless::solami::solana::storage::confirmed_block::{
        CompiledInstruction, InnerInstruction, InnerInstructions, Message, Transaction,
        TransactionError, TransactionStatusMeta, UiTokenAmount,
    };
    use relayscope_core::{TxView, decode_transaction};

    /// Builds the streamed form of a JSON-RPC transaction, the reverse of [`rpc_json`].
    fn stream_from_rpc(raw: &Value) -> SubscribeUpdateTransactionInfo {
        let bytes = |s: &Value| bs58::decode(s.as_str().unwrap()).into_vec().unwrap();
        let keys = |v: &Value| v.as_array().unwrap().iter().map(bytes).collect::<Vec<_>>();
        let indexes = |v: &Value| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_u64().unwrap() as u8)
                .collect::<Vec<u8>>()
        };
        let balances = |v: &Value| {
            v.as_array()
                .map(|list| {
                    list.iter()
                        .map(|b| TokenBalance {
                            account_index: b["accountIndex"].as_u64().unwrap() as u32,
                            mint: b["mint"].as_str().unwrap().into(),
                            owner: b["owner"].as_str().unwrap_or_default().into(),
                            ui_token_amount: Some(UiTokenAmount {
                                amount: b["uiTokenAmount"]["amount"].as_str().unwrap().into(),
                                decimals: b["uiTokenAmount"]["decimals"].as_u64().unwrap() as u32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let m = &raw["meta"];
        let message = &raw["transaction"]["message"];
        SubscribeUpdateTransactionInfo {
            signature: bytes(&raw["transaction"]["signatures"][0]),
            transaction: Some(Transaction {
                signatures: keys(&raw["transaction"]["signatures"]),
                message: Some(Message {
                    account_keys: keys(&message["accountKeys"]),
                    instructions: message["instructions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|ix| CompiledInstruction {
                            program_id_index: ix["programIdIndex"].as_u64().unwrap() as u32,
                            accounts: indexes(&ix["accounts"]),
                            data: bytes(&ix["data"]),
                        })
                        .collect(),
                    ..Default::default()
                }),
            }),
            meta: Some(TransactionStatusMeta {
                err: (!m["err"].is_null()).then(|| TransactionError { err: vec![1] }),
                inner_instructions: m["innerInstructions"]
                    .as_array()
                    .map(|groups| {
                        groups
                            .iter()
                            .map(|g| InnerInstructions {
                                index: g["index"].as_u64().unwrap() as u32,
                                instructions: g["instructions"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|ix| InnerInstruction {
                                        program_id_index: ix["programIdIndex"].as_u64().unwrap()
                                            as u32,
                                        accounts: indexes(&ix["accounts"]),
                                        data: bytes(&ix["data"]),
                                        stack_height: ix["stackHeight"].as_u64().map(|h| h as u32),
                                    })
                                    .collect(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                loaded_writable_addresses: m["loadedAddresses"]["writable"]
                    .as_array()
                    .map(|_| keys(&m["loadedAddresses"]["writable"]))
                    .unwrap_or_default(),
                loaded_readonly_addresses: m["loadedAddresses"]["readonly"]
                    .as_array()
                    .map(|_| keys(&m["loadedAddresses"]["readonly"]))
                    .unwrap_or_default(),
                pre_token_balances: balances(&m["preTokenBalances"]),
                post_token_balances: balances(&m["postTokenBalances"]),
                log_messages: m["logMessages"]
                    .as_array()
                    .map(|l| l.iter().map(|s| s.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn every_fixture_decodes_the_same_from_the_stream_as_from_rpc() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let raw: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let from_rpc = decode_transaction(&TxView::from_rpc_json(&raw).unwrap()).unwrap();
            let streamed = stream_from_rpc(&raw);
            let rebuilt = rpc_json(raw["slot"].as_u64().unwrap(), &streamed).unwrap();
            let from_stream =
                decode_transaction(&TxView::from_rpc_json(&rebuilt).unwrap()).unwrap();
            // The stream carries no block time; everything else must be identical.
            let mut expected = from_rpc.clone();
            expected.block_time = None;
            assert_eq!(from_stream, expected, "{}", path.display());
            checked += 1;
        }
        assert_eq!(checked, 18);
    }
}
