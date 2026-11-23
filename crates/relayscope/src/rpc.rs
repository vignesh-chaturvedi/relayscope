//! A small Solana JSON-RPC client with a rate limit and retries.
//!
//! Some providers put the API key in the URL, so the URL is never printed: transport errors
//! are stripped of it before they are returned.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::store::SignatureRow;

pub const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";

/// JSON-RPC error codes that mean "slow down" rather than "this request is wrong". Some
/// providers return them inside an HTTP 200 reply.
const RATE_LIMIT_CODES: &[i64] = &[-32005, -32429];
const MAX_ATTEMPTS: u32 = 8;

/// Requests are spaced `interval` apart. Every rate-limit reply doubles the interval, and
/// every success shrinks it by a tenth, down to the configured rate. A throttled endpoint is
/// slowed down to what it accepts instead of being hammered with retries.
struct Pace {
    next: Instant,
    interval: Duration,
}

const SLOWEST: Duration = Duration::from_secs(10);

pub struct Rpc {
    url: String,
    http: reqwest::Client,
    pace: Mutex<Pace>,
    fastest: Duration,
    requests: AtomicU64,
    throttled: AtomicU64,
}

/// Request counts since the client was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RpcCounts {
    pub requests: u64,
    pub throttled: u64,
}

impl Rpc {
    /// `requests_per_second` is shared by every request made through this client.
    pub fn new(url: impl Into<String>, requests_per_second: f64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("a default HTTP client builds");
        let fastest = Duration::from_secs_f64(1.0 / requests_per_second.max(0.01));
        Self {
            url: url.into(),
            http,
            pace: Mutex::new(Pace {
                next: Instant::now(),
                interval: fastest,
            }),
            fastest,
            requests: AtomicU64::new(0),
            throttled: AtomicU64::new(0),
        }
    }

    pub fn counts(&self) -> RpcCounts {
        RpcCounts {
            requests: self.requests.load(Ordering::Relaxed),
            throttled: self.throttled.load(Ordering::Relaxed),
        }
    }

    /// The current spacing between requests, after any slowdown.
    pub async fn current_rate(&self) -> f64 {
        1.0 / self.pace.lock().await.interval.as_secs_f64()
    }

    async fn on_throttled(&self) {
        self.throttled.fetch_add(1, Ordering::Relaxed);
        let mut pace = self.pace.lock().await;
        pace.interval = (pace.interval * 2).min(SLOWEST);
    }

    async fn on_success(&self) {
        let mut pace = self.pace.lock().await;
        pace.interval = pace.interval.mul_f64(0.9).max(self.fastest);
    }

    /// The endpoint's host, safe to print.
    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown host".into())
    }

    async fn wait_turn(&self) {
        let start = {
            let mut pace = self.pace.lock().await;
            let start = pace.next.max(Instant::now());
            pace.next = start + pace.interval;
            start
        };
        tokio::time::sleep_until(start.into()).await;
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Calls a method, retrying rate limits, server errors and network errors with backoff.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut delay = Duration::from_millis(500);
        let mut last_error = anyhow!("{method}: no attempt made");
        for _ in 0..MAX_ATTEMPTS {
            self.wait_turn().await;
            match self.http.post(&self.url).json(&body).send().await {
                Ok(response) if response.status().is_success() => {
                    let mut reply: Value = response
                        .json()
                        .await
                        .map_err(|e| anyhow!("{method}: reply is not JSON: {}", e.without_url()))?;
                    match reply.get("error") {
                        None => {
                            self.on_success().await;
                            return Ok(reply["result"].take());
                        }
                        Some(error) if is_rate_limit(error) => {
                            self.on_throttled().await;
                            last_error = anyhow!("{method}: rate limited: {error}");
                        }
                        Some(error) => bail!("{method} failed: {error}"),
                    }
                }
                Ok(response) if response.status().as_u16() == 429 => {
                    self.on_throttled().await;
                    last_error = anyhow!("{method}: HTTP {}", response.status());
                }
                Ok(response) if response.status().is_server_error() => {
                    last_error = anyhow!("{method}: HTTP {}", response.status());
                }
                Ok(response) => bail!("{method}: HTTP {}", response.status()),
                Err(error) => last_error = anyhow!("{method}: {}", error.without_url()),
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(16));
        }
        Err(last_error).context(format!(
            "{method} kept failing after {MAX_ATTEMPTS} attempts"
        ))
    }

    /// `getTransaction` at finalized commitment, in the format
    /// [`relayscope_core::TxView::from_rpc_json`] reads.
    pub async fn transaction(&self, signature: &str) -> Result<Value> {
        let result = self
            .call(
                "getTransaction",
                json!([signature, {
                    "encoding": "json",
                    "maxSupportedTransactionVersion": 0,
                    "commitment": "finalized",
                }]),
            )
            .await?;
        if result.is_null() {
            bail!(
                "transaction {signature} not found (not finalized yet, or the endpoint keeps no history that old)"
            );
        }
        Ok(result)
    }

    /// One page of `getSignaturesForAddress` at finalized commitment, newest first.
    pub async fn signatures_for_address(
        &self,
        address: &str,
        before: Option<&str>,
        until: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SignatureRow>> {
        let mut options = json!({ "limit": limit, "commitment": "finalized" });
        if let Some(before) = before {
            options["before"] = json!(before);
        }
        if let Some(until) = until {
            options["until"] = json!(until);
        }
        let result = self
            .call("getSignaturesForAddress", json!([address, options]))
            .await?;
        let rows = result
            .as_array()
            .context("getSignaturesForAddress: expected an array")?;
        rows.iter()
            .map(|row| {
                Ok(SignatureRow {
                    signature: row["signature"]
                        .as_str()
                        .context("row without a signature")?
                        .to_owned(),
                    slot: row["slot"].as_u64().context("row without a slot")?,
                    block_time: row["blockTime"].as_i64(),
                    failed: !row["err"].is_null(),
                })
            })
            .collect()
    }

    /// Raw account data, or `None` if the account does not exist.
    pub async fn account_data(&self, address: &str) -> Result<Option<Vec<u8>>> {
        use base64::Engine;
        let result = self
            .call(
                "getAccountInfo",
                json!([address, { "encoding": "base64", "commitment": "finalized" }]),
            )
            .await?;
        let Some(data) = result["value"]["data"][0].as_str() else {
            return Ok(None);
        };
        Ok(Some(
            base64::engine::general_purpose::STANDARD.decode(data)?,
        ))
    }
}

fn is_rate_limit(error: &Value) -> bool {
    let code_matches = error["code"]
        .as_i64()
        .is_some_and(|c| RATE_LIMIT_CODES.contains(&c));
    let message = error["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    code_matches || message.contains("rate limit") || message.contains("too many requests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_rate_limits_in_a_200_reply() {
        assert!(is_rate_limit(
            &json!({"code": -32005, "message": "Too many requests for a specific RPC call"})
        ));
        assert!(is_rate_limit(
            &json!({"code": -32000, "message": "Rate limit exceeded"})
        ));
        assert!(!is_rate_limit(
            &json!({"code": -32602, "message": "Invalid param: WrongSize"})
        ));
    }

    #[test]
    fn host_hides_the_query_string() {
        let rpc = Rpc::new("https://rpc.example.com/sol?api_key=secret", 1.0);
        assert_eq!(rpc.host(), "rpc.example.com");
    }

    #[tokio::test]
    async fn throttling_slows_down_and_success_recovers() {
        let rpc = Rpc::new("http://127.0.0.1:9", 4.0);
        assert_eq!(rpc.current_rate().await, 4.0);
        rpc.on_throttled().await;
        rpc.on_throttled().await;
        assert_eq!(rpc.current_rate().await, 1.0);
        for _ in 0..50 {
            rpc.on_success().await;
        }
        assert_eq!(
            rpc.current_rate().await,
            4.0,
            "recovers to the configured rate, never above it"
        );
        assert_eq!(rpc.counts().throttled, 2);
    }

    #[tokio::test]
    async fn rate_limit_spaces_requests() {
        let rpc = Rpc::new("http://127.0.0.1:9", 20.0);
        let start = Instant::now();
        for _ in 0..5 {
            rpc.wait_turn().await;
        }
        // Five turns at 20 per second: the fifth starts at least 200 ms after the first.
        assert!(start.elapsed() >= Duration::from_millis(195));
    }
}
