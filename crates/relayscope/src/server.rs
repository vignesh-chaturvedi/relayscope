//! `relayscope serve`: the JSON API, a live feed of new donations over server-sent events,
//! the stream overlay, signed webhooks when configured, and the web app.
//!
//! Every request opens its own read-only SQLite connection on a blocking thread, so the
//! server can run beside `relayscope live`, which writes to the same database.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures::Stream;
use futures::StreamExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::{self, DonationBrief, Tokens};
use crate::webhooks;

pub struct ServeOptions {
    pub listen: SocketAddr,
    pub db: PathBuf,
    pub web: PathBuf,
    /// Base URL of the operator's public API, for charity names. `None` disables labels.
    pub labels_url: Option<String>,
    /// Webhook endpoints. `None` sends no webhooks.
    pub webhooks: Option<webhooks::Config>,
}

/// What the live feed carries: stored donations, and test alerts for one overlay.
#[derive(Clone)]
enum FeedItem {
    Donation(Box<DonationBrief>),
    Test { config_id: String, key: String },
}

/// An overlay's config ID and test key.
type OverlayKey = (String, String);

struct AppState {
    db: PathBuf,
    tokens: RwLock<Tokens>,
    feed: broadcast::Sender<FeedItem>,
    labels: Arc<Labels>,
    overview: Mutex<Option<(Instant, Value)>>,
    /// Open overlays by config ID and test key, so a test alert can say who saw it.
    overlays: Mutex<HashMap<OverlayKey, usize>>,
    /// When each test key last sent a test alert.
    tests_sent: Mutex<HashMap<String, Instant>>,
}

type Shared = Arc<AppState>;

fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
    }
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

/// A config ID in hex or base58, as hex.
fn config_hex(id: &str) -> Result<String, ApiError> {
    id.parse::<relayscope_core::Bytes32>()
        .map(|b| b.to_hex())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "not a config ID".into()))
}

/// Test keys are random strings the setup page makes, so only the streamer can send tests.
fn valid_key(key: &str) -> bool {
    (16..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Runs a query on a blocking thread with its own read-only connection.
async fn query<T: Send + 'static>(
    state: &Shared,
    f: impl FnOnce(&Connection, &Tokens) -> Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    let path = state.db.clone();
    let tokens = state.tokens.read().expect("tokens lock").clone();
    tokio::task::spawn_blocking(move || {
        let conn = open_read_only(&path)?;
        f(&conn, &tokens)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(ApiError::from)
}

pub async fn run(options: ServeOptions) -> Result<()> {
    let tokens = {
        let conn = open_read_only(&options.db)?;
        Tokens::load(&conn)?
    };
    let (feed, _) = broadcast::channel(1_024);
    let state = Arc::new(AppState {
        labels: Arc::new(Labels::new(options.labels_url.clone(), options.db.clone())),
        db: options.db.clone(),
        tokens: RwLock::new(tokens),
        feed,
        overview: Mutex::new(None),
        overlays: Mutex::new(HashMap::new()),
        tests_sent: Mutex::new(HashMap::new()),
    });
    tokio::spawn(watch_donations(state.clone()));
    tokio::spawn(refresh_tokens(state.clone()));
    let endpoints = options.webhooks.as_ref().map_or(0, |w| w.endpoints.len());
    if let Some(config) = options.webhooks {
        let db = options.db.clone();
        tokio::spawn(async move {
            if let Err(error) = webhooks::run(db, Arc::new(config)).await {
                eprintln!("webhooks stopped: {error:#}");
            }
        });
    }

    let index = options.web.join("index.html");
    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/overview", get(overview))
        .route("/api/v1/tx/{signature}", get(transaction))
        .route("/api/v1/epoch/{config}/{mint}/{epoch}", get(epoch))
        .route("/api/v1/config/{config}", get(config))
        .route("/api/v1/address/{address}", get(address))
        .route("/api/v1/label/{config}", get(label))
        .route("/api/v1/search", get(search))
        .route("/api/v1/checks", get(checks))
        .route("/api/v1/stream", get(stream))
        .route("/api/v1/overlay/test", post(overlay_test))
        .route_service(
            "/overlay/{config}",
            ServeFile::new(options.web.join("overlay.html")),
        )
        .fallback_service(ServeDir::new(&options.web).fallback(ServeFile::new(index)))
        .layer(middleware::from_fn(timing))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    eprintln!(
        "serving on http://{} (web app from {}, labels {}, webhooks to {endpoints} {})",
        options.listen,
        options.web.display(),
        if options.labels_url.is_some() {
            "on"
        } else {
            "off"
        },
        if endpoints == 1 {
            "endpoint"
        } else {
            "endpoints"
        }
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Adds `Server-Timing` to every response, so the time spent in the server is visible.
async fn timing(request: Request, next: Next) -> Response {
    let start = Instant::now();
    let mut response = next.run(request).await;
    let ms = start.elapsed().as_secs_f64() * 1_000.0;
    if let Ok(value) = HeaderValue::from_str(&format!("app;dur={ms:.1}")) {
        response.headers_mut().insert("server-timing", value);
    }
    response
}

/// Polls for new donations twice a second and broadcasts them to live-feed subscribers.
async fn watch_donations(state: Shared) {
    let mut last_slot: i64 = match query(&state, |c, _| api::latest_slot(c)).await {
        Ok(slot) => slot,
        Err(ApiError(_, e)) => {
            eprintln!("live feed: could not read the latest slot: {e}");
            0
        }
    };
    // Donations already sent at the newest slot, so a slot is never sent twice.
    let mut sent_at_last: HashSet<(String, i64)> = HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        let from = last_slot - 1;
        let Ok(rows) = query(&state, move |c, t| api::donations_after(c, t, from)).await else {
            continue;
        };
        for d in rows {
            let key = (d.signature.clone(), d.ordinal);
            if d.slot < last_slot || (d.slot == last_slot && sent_at_last.contains(&key)) {
                continue;
            }
            if d.slot > last_slot {
                last_slot = d.slot;
                sent_at_last.clear();
            }
            sent_at_last.insert(key);
            let _ = state.feed.send(FeedItem::Donation(Box::new(d)));
        }
    }
}

async fn refresh_tokens(state: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    tick.tick().await;
    loop {
        tick.tick().await;
        if let Ok(tokens) = query(&state, |c, _| Tokens::load(c)).await {
            *state.tokens.write().expect("tokens lock") = tokens;
        }
    }
}

async fn health(State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    let indexing = query(&state, |c, _| api::indexing(c)).await?;
    Ok(Json(json!({ "ok": true, "indexing": indexing })))
}

async fn overview(State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    // The overview scans every donation, so it is cached for a few seconds.
    if let Some((at, value)) = state.overview.lock().expect("overview lock").as_ref()
        && at.elapsed() < Duration::from_secs(10)
    {
        return Ok(Json(value.clone()));
    }
    let o = query(&state, api::overview).await?;
    let value = serde_json::to_value(o).map_err(|e| anyhow::anyhow!(e))?;
    *state.overview.lock().expect("overview lock") = Some((Instant::now(), value.clone()));
    Ok(Json(value))
}

#[derive(Serialize)]
struct Journey {
    donation: DonationBrief,
    epoch: Option<api::Epoch>,
    share: Option<api::Share>,
    label: Option<ConfigLabel>,
}

async fn transaction(
    State(state): State<Shared>,
    UrlPath(signature): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let sig = signature.clone();
    let tx = query(&state, move |c, t| api::transaction(c, t, &sig))
        .await?
        .ok_or_else(|| not_found("transaction"))?;
    let mut journeys = Vec::new();
    for d in &tx.donations {
        let (config, mint, n) = (d.pool.config_id.clone(), d.pool.mint.clone(), d.pool.epoch);
        let epoch = query(&state, move |c, t| api::epoch(c, t, &config, &mint, n)).await?;
        let share = epoch.as_ref().and_then(|e| api::share(d, e));
        let label = state.labels.cached(&d.pool.config_id);
        journeys.push(Journey {
            donation: d.clone(),
            epoch,
            share,
            label,
        });
    }
    Ok(Json(json!({ "transaction": tx, "journeys": journeys })))
}

async fn epoch(
    State(state): State<Shared>,
    UrlPath((config, mint, n)): UrlPath<(String, String, i64)>,
) -> Result<Json<Value>, ApiError> {
    let id = config.clone();
    let e = query(&state, move |c, t| api::epoch(c, t, &config, &mint, n))
        .await?
        .ok_or_else(|| not_found("epoch"))?;
    let label = state.labels.cached(&id);
    Ok(Json(json!({ "epoch": e, "label": label })))
}

async fn config(
    State(state): State<Shared>,
    UrlPath(config): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let hex = config_hex(&config)?;
    let id = hex.clone();
    let c = query(&state, move |c, t| api::config(c, t, &hex))
        .await?
        .ok_or_else(|| not_found("config"))?;
    let label = state.labels.cached(&id);
    Ok(Json(json!({ "config": c, "label": label })))
}

async fn address(
    State(state): State<Shared>,
    UrlPath(address): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    address
        .parse::<relayscope_core::Pubkey>()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "not an address".into()))?;
    let a = query(&state, move |c, t| api::address(c, t, &address)).await?;
    Ok(Json(json!({ "address": a })))
}

/// Beneficiary names for one config, fetched if needed. Pages load without waiting for this.
async fn label(
    State(state): State<Shared>,
    UrlPath(config): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let hex = config_hex(&config)?;
    Ok(Json(json!({ "label": state.labels.get(&hex).await })))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

async fn search(
    State(state): State<Shared>,
    Query(params): Query<SearchParams>,
) -> Result<Json<api::Found>, ApiError> {
    Ok(Json(
        query(&state, move |c, _| api::search(c, &params.q)).await?,
    ))
}

async fn checks(State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    let (summary, indexing) = query(&state, |c, _| {
        Ok((api::check_summary(c)?, api::indexing(c)?))
    })
    .await?;
    Ok(Json(json!({ "checks": summary, "indexing": indexing })))
}

#[derive(Deserialize)]
struct StreamParams {
    config: Option<String>,
    /// An overlay's test key, to receive its test alerts.
    key: Option<String>,
}

/// Keeps the count of open overlays right: one is counted while its stream is open.
struct OverlayGuard {
    state: Shared,
    key: OverlayKey,
}

impl OverlayGuard {
    fn new(state: &Shared, key: OverlayKey) -> Self {
        *state
            .overlays
            .lock()
            .expect("overlays lock")
            .entry(key.clone())
            .or_default() += 1;
        Self {
            state: state.clone(),
            key,
        }
    }
}

impl Drop for OverlayGuard {
    fn drop(&mut self) {
        let mut overlays = self.state.overlays.lock().expect("overlays lock");
        if let Some(n) = overlays.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                overlays.remove(&self.key);
            }
        }
    }
}

fn donation_event(d: &DonationBrief) -> Option<Event> {
    let data = serde_json::to_string(d).ok()?;
    Some(
        Event::default()
            .event("donation")
            .id(d.slot.to_string())
            .data(data),
    )
}

/// New donations as they are stored, as `donation` events whose ID is the slot. `?config=`
/// limits it to one config, and then a client that reconnects with `Last-Event-ID` first gets
/// what it missed. `?key=` adds `test` events for that overlay.
async fn stream(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(params): Query<StreamParams>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // A bad config ID is refused, so a mistyped overlay URL never shows every donation.
    let config = params.config.as_deref().map(config_hex).transpose()?;
    let key = params.key.filter(|k| valid_key(k));
    let guard = match (&config, &key) {
        (Some(c), Some(k)) => Some(OverlayGuard::new(&state, (c.clone(), k.clone()))),
        _ => None,
    };
    // Subscribe before reading what was missed, so nothing falls between the two.
    let receiver = state.feed.subscribe();
    let last_slot = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let missed = match (&config, last_slot) {
        (Some(c), Some(slot)) => {
            let c = c.clone();
            query(&state, move |conn, t| {
                api::config_donations_since(conn, t, &c, slot, 20)
            })
            .await
            .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let replayed: HashSet<(String, i64)> = missed
        .iter()
        .map(|d| (d.signature.clone(), d.ordinal))
        .collect();
    let head = futures::stream::iter(
        missed
            .iter()
            .filter_map(donation_event)
            .map(Ok)
            .collect::<Vec<_>>(),
    );
    let live = BroadcastStream::new(receiver).filter_map(move |item| {
        let _open = &guard;
        let event = match item {
            Ok(FeedItem::Donation(d)) => {
                let other_config = config.as_ref().is_some_and(|c| *c != d.pool.config_id);
                if other_config || replayed.contains(&(d.signature.clone(), d.ordinal)) {
                    None
                } else {
                    donation_event(&d)
                }
            }
            Ok(FeedItem::Test { config_id, key: k })
                if config.as_ref() == Some(&config_id) && key.as_ref() == Some(&k) =>
            {
                Some(Event::default().event("test").data("{}"))
            }
            _ => None,
        };
        async move { event.map(Ok) }
    });
    Ok(Sse::new(head.chain(live)).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
struct TestAlert {
    config: String,
    key: String,
}

/// Sends a test alert to the overlays open with this config and key, at most one every two
/// seconds per key. Answers with how many overlays are open.
async fn overlay_test(
    State(state): State<Shared>,
    Json(body): Json<TestAlert>,
) -> Result<Json<Value>, ApiError> {
    let config_id = config_hex(&body.config)?;
    if !valid_key(&body.key) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "the key must be 16 to 64 letters, digits, - or _".into(),
        ));
    }
    {
        let now = Instant::now();
        let mut sent = state.tests_sent.lock().expect("tests lock");
        if sent
            .get(&body.key)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(2))
        {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "wait a moment before sending another test alert".into(),
            ));
        }
        if sent.len() > 1_000 {
            sent.retain(|_, at| now.duration_since(*at) < Duration::from_secs(60));
        }
        sent.insert(body.key.clone(), now);
    }
    let overlays = state
        .overlays
        .lock()
        .expect("overlays lock")
        .get(&(config_id.clone(), body.key.clone()))
        .copied()
        .unwrap_or(0);
    let _ = state.feed.send(FeedItem::Test {
        config_id,
        key: body.key,
    });
    Ok(Json(json!({ "overlays": overlays })))
}

/// Beneficiary names for a config, from the operator's public API. Shown as labels, never
/// used as proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigLabel {
    pub beneficiaries: Vec<Beneficiary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Beneficiary {
    pub name: String,
    pub kind: Option<String>,
    pub weight: Option<String>,
}

const LABEL_TTL: i64 = 24 * 3_600;
const LABEL_RETRY_AFTER: i64 = 600;

struct Labels {
    base: Option<String>,
    /// Config IDs being fetched in the background, so each is fetched once at a time.
    pending: Mutex<HashSet<String>>,
    db: PathBuf,
    http: reqwest::Client,
    /// Config ID to label (or `None` if the API had none) and when it was fetched.
    memory: Mutex<HashMap<String, (Option<ConfigLabel>, i64)>>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Labels {
    fn new(base: Option<String>, db: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(1_500))
            .build()
            .expect("a default HTTP client builds");
        Self {
            base: base.map(|b| b.trim_end_matches('/').to_string()),
            pending: Mutex::new(HashSet::new()),
            db,
            http,
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// A label only if it is already cached, without waiting on the network. On a miss, it is
    /// fetched in the background for next time.
    fn cached(self: &Arc<Self>, config_id: &str) -> Option<ConfigLabel> {
        self.base.as_ref()?;
        let now = unix_now();
        if let Some((label, at)) = self
            .memory
            .lock()
            .expect("labels lock")
            .get(config_id)
            .cloned()
            && now - at < LABEL_TTL
        {
            return label;
        }
        if let Some((label, at)) = self.read_cached(config_id)
            && now - at < LABEL_TTL
        {
            self.memory
                .lock()
                .expect("labels lock")
                .insert(config_id.into(), (label.clone(), at));
            return label;
        }
        if self
            .pending
            .lock()
            .expect("labels lock")
            .insert(config_id.to_string())
        {
            let labels = Arc::clone(self);
            let id = config_id.to_string();
            tokio::spawn(async move {
                labels.get(&id).await;
                labels.pending.lock().expect("labels lock").remove(&id);
            });
        }
        None
    }

    /// A cached label, fetched on first use. A slow or failing API returns `None` quickly.
    async fn get(&self, config_id: &str) -> Option<ConfigLabel> {
        let base = self.base.as_ref()?;
        let now = unix_now();
        if let Some((label, at)) = self
            .memory
            .lock()
            .expect("labels lock")
            .get(config_id)
            .cloned()
        {
            let fresh = if label.is_some() {
                now - at < LABEL_TTL
            } else {
                now - at < LABEL_RETRY_AFTER
            };
            if fresh {
                return label;
            }
        }
        if let Some((label, at)) = self.read_cached(config_id)
            && now - at < LABEL_TTL
        {
            self.memory
                .lock()
                .expect("labels lock")
                .insert(config_id.into(), (label.clone(), at));
            return label;
        }
        let label = self.fetch(base, config_id).await;
        self.memory
            .lock()
            .expect("labels lock")
            .insert(config_id.into(), (label.clone(), now));
        if label.is_some() {
            self.write_cached(config_id, label.as_ref(), now);
        }
        label
    }

    async fn fetch(&self, base: &str, config_id: &str) -> Option<ConfigLabel> {
        let url = format!("{base}/v2/configs/{config_id}");
        let body: Value = self
            .http
            .get(url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        let beneficiaries = body["beneficiaries"]
            .as_array()?
            .iter()
            .filter_map(|b| {
                Some(Beneficiary {
                    name: b["name"].as_str()?.to_string(),
                    kind: b["type"].as_str().map(str::to_owned),
                    weight: b["weight"].as_str().map(str::to_owned),
                })
            })
            .collect();
        Some(ConfigLabel { beneficiaries })
    }

    fn read_cached(&self, config_id: &str) -> Option<(Option<ConfigLabel>, i64)> {
        let conn = open_read_only(&self.db).ok()?;
        let (data, at): (Option<String>, i64) = conn
            .query_row(
                "SELECT data, fetched_at FROM labels WHERE config_id = ?1",
                [config_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()??;
        Some((data.and_then(|d| serde_json::from_str(&d).ok()), at))
    }

    fn write_cached(&self, config_id: &str, label: Option<&ConfigLabel>, at: i64) {
        let write = || -> Result<()> {
            let conn = Connection::open(&self.db)?;
            conn.busy_timeout(Duration::from_secs(5))?;
            conn.execute(
                "INSERT INTO labels (config_id, data, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (config_id) DO UPDATE SET data = excluded.data, fetched_at = excluded.fetched_at",
                params![config_id, label.map(|l| serde_json::to_string(l).unwrap_or_default()), at],
            )?;
            Ok(())
        };
        if let Err(error) = write() {
            eprintln!("labels: could not cache {config_id}: {error:#}");
        }
    }
}
