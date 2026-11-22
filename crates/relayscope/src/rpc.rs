//! A small Solana JSON-RPC client with retries.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";

pub struct Rpc {
    url: String,
    http: reqwest::Client,
}

impl Rpc {
    pub fn new(url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("a default HTTP client builds");
        Self {
            url: url.into(),
            http,
        }
    }

    /// Calls a method, retrying rate limits, server errors and timeouts with backoff.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut delay = Duration::from_millis(500);
        let mut last_error = None;
        for _ in 0..6 {
            match self.http.post(&self.url).json(&body).send().await {
                Ok(response) if response.status().is_success() => {
                    let mut reply: Value =
                        response.json().await.context("RPC reply is not JSON")?;
                    if let Some(error) = reply.get("error") {
                        bail!("{method} failed: {error}");
                    }
                    return Ok(reply["result"].take());
                }
                Ok(response)
                    if response.status().as_u16() == 429 || response.status().is_server_error() =>
                {
                    last_error = Some(anyhow::anyhow!("{method}: HTTP {}", response.status()));
                }
                Ok(response) => bail!("{method}: HTTP {}", response.status()),
                Err(error) => last_error = Some(error.into()),
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(8));
        }
        Err(last_error.expect("the loop ran at least once"))
            .context(format!("{method} kept failing"))
    }

    /// `getTransaction` in the format [`relayscope_core::TxView::from_rpc_json`] reads.
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
            bail!("transaction {signature} not found (it may not be finalized yet)");
        }
        Ok(result)
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
