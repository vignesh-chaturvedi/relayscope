//! Backfill: list every signature that touched the program, then fetch, decode and store
//! each transaction. Both steps resume where they stopped.

use std::time::Instant;

use anyhow::{Result, bail};
use futures::{StreamExt, stream};
use relayscope_core::PROGRAM_ID;

use crate::rpc::Rpc;
use crate::store::{Origin, Store, decode_fetched};

const PAGE: usize = 1_000;

#[derive(Debug, Default)]
pub struct ListReport {
    pub pages: u64,
    pub added: u64,
}

/// Lists signatures into the work queue: first back through history until an empty page,
/// then forward from the head down to the newest signature already listed.
pub async fn list(rpc: &Rpc, store: &mut Store) -> Result<ListReport> {
    let program = PROGRAM_ID.to_string();
    let mut report = ListReport::default();

    if store.state("history_complete")?.is_none() {
        let mut before = store.state("oldest_listed")?;
        loop {
            let page = rpc
                .signatures_for_address(&program, before.as_deref(), None, PAGE)
                .await?;
            let Some(last) = page.last() else {
                store.set_state("history_complete", "1")?;
                break;
            };
            if store.state("newest_listed")?.is_none() {
                store.set_state("newest_listed", &page[0].signature)?;
            }
            let last = last.clone();
            report.added += store.insert_signatures(&page)? as u64;
            store.set_state("oldest_listed", &last.signature)?;
            report.pages += 1;
            if report.pages % 20 == 0 {
                eprintln!(
                    "  listed {} pages, back to {}",
                    report.pages,
                    date(last.block_time)
                );
            }
            before = Some(last.signature);
        }
    }

    if let Some(newest) = store.state("newest_listed")? {
        let mut before: Option<String> = None;
        let mut head: Option<String> = None;
        loop {
            let page = rpc
                .signatures_for_address(&program, before.as_deref(), Some(&newest), PAGE)
                .await?;
            let Some(last) = page.last() else { break };
            before = Some(last.signature.clone());
            head.get_or_insert_with(|| page[0].signature.clone());
            report.added += store.insert_signatures(&page)? as u64;
            report.pages += 1;
        }
        if let Some(head) = head {
            store.set_state("newest_listed", &head)?;
        }
    }
    Ok(report)
}

pub struct FetchOptions {
    pub concurrency: usize,
    pub batch: usize,
    /// Stop after this many transactions, for trial runs.
    pub max: Option<u64>,
}

#[derive(Debug, Default)]
pub struct FetchReport {
    pub stored: u64,
    pub decode_failures: u64,
    pub fetch_failures: u64,
}

/// Fetches every listed transaction not yet stored, oldest first.
///
/// Requests flow continuously, `concurrency` at a time, and finished ones are written in
/// chunks of `batch`, each in one SQLite transaction. A slow or throttled request holds only
/// its own slot, never a whole batch, and stopping at any point loses at most one chunk.
pub async fn fetch(rpc: &Rpc, store: &mut Store, options: &FetchOptions) -> Result<FetchReport> {
    let mut queue = store.unfetched(usize::MAX)?;
    if let Some(max) = options.max {
        queue.truncate(usize::try_from(max).unwrap_or(usize::MAX));
    }
    let total = queue.len() as u64;
    let started = Instant::now();
    let mut report = FetchReport::default();
    let mut chunks_without_success = 0;

    let mut chunks = stream::iter(queue)
        .map(|signature| async move {
            let result = rpc.transaction(&signature).await;
            (signature, result)
        })
        .buffer_unordered(options.concurrency.max(1))
        .chunks(options.batch.max(1));

    while let Some(results) = chunks.next().await {
        let mut items = Vec::with_capacity(results.len());
        for (signature, result) in results {
            match result {
                Ok(raw) => {
                    let item = decode_fetched(signature, raw);
                    if let Err(error) = &item.decoded {
                        report.decode_failures += 1;
                        eprintln!("  decode failure {}: {error}", item.signature);
                    }
                    items.push(item);
                }
                Err(error) => {
                    // Left in the queue for the next run.
                    report.fetch_failures += 1;
                    eprintln!("  could not fetch {signature}: {error:#}");
                }
            }
        }
        if items.is_empty() {
            chunks_without_success += 1;
            if chunks_without_success >= 3 {
                bail!("three chunks in a row could not be fetched; stopping (errors above)");
            }
            continue;
        }
        chunks_without_success = 0;
        store.write_fetched(&items, Origin::Backfill)?;
        report.stored += items.len() as u64;

        let rate = report.stored as f64 / started.elapsed().as_secs_f64().max(0.001);
        let left = total.saturating_sub(report.stored + report.fetch_failures);
        let counts = rpc.counts();
        eprintln!(
            "  stored {}/{total} ({rate:.1}/s, about {} left), pacing {:.1} req/s, throttled {} of {} requests, decode failures {}, fetch failures {}",
            report.stored,
            duration(left as f64 / rate.max(0.001)),
            rpc.current_rate().await,
            counts.throttled,
            counts.requests,
            report.decode_failures,
            report.fetch_failures
        );
    }
    Ok(report)
}

fn date(block_time: Option<i64>) -> String {
    block_time
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "an unknown date".into())
}

fn duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(42.0), "42s");
        assert_eq!(duration(600.0), "10m");
        assert_eq!(duration(69_400.0), "19h 16m");
    }
}
