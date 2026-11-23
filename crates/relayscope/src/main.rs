mod api;
mod backfill;
mod idl;
mod known;
mod live;
mod rpc;
mod server;
mod stats;
mod store;
mod verify;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use relayscope_core::{DecodedTx, TxView, decode_transaction};

use crate::rpc::{DEFAULT_RPC_URL, Rpc};
use crate::store::Store;

/// Real-time tracker for the Donation Relay program on Solana.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Solana JSON-RPC endpoint. Backfill needs one that keeps full history.
    #[arg(long, global = true, env = "RELAYSCOPE_RPC_URL", default_value = DEFAULT_RPC_URL, hide_env_values = true)]
    rpc_url: String,

    /// Requests per second to send, across all concurrent requests. Solana's public endpoint
    /// allows about 4 per second for one method.
    #[arg(long, global = true, env = "RELAYSCOPE_RPS", default_value_t = 3.5)]
    rps: f64,

    /// SQLite database file.
    #[arg(
        long,
        global = true,
        env = "RELAYSCOPE_DB",
        default_value = "data/relayscope.db"
    )]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List every signature that touched the program, then fetch, decode and store each
    /// transaction. Safe to stop and rerun: it resumes where it stopped.
    Backfill {
        /// Requests in flight at once.
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
        /// Transactions written per SQLite transaction.
        #[arg(long, default_value_t = 50)]
        batch: usize,
        /// Stop after storing this many transactions.
        #[arg(long)]
        max: Option<u64>,
        /// Only list signatures; fetch nothing.
        #[arg(long)]
        list_only: bool,
        /// Skip listing and only fetch what is already queued.
        #[arg(long, conflicts_with = "list_only")]
        skip_list: bool,
    },
    /// Follow the program in real time through gapless and Solami's gRPC stream. Runs until
    /// stopped. With --kill-test, cuts the stream on purpose, verifies the recovery against RPC,
    /// writes a report and exits with 1 if anything was missed.
    Live {
        /// Where new transactions come from: a Solana RPC WebSocket, or Solami's gRPC stream
        /// through gapless (needs a Solami plan with gRPC access).
        #[arg(long, value_enum, default_value_t = LiveSource::Websocket)]
        source: LiveSource,
        /// WebSocket URL. Defaults to the RPC URL with https:// replaced by wss://.
        #[arg(long, env = "RELAYSCOPE_WS_URL", hide_env_values = true)]
        ws_url: Option<String>,
        #[arg(long, env = "SOLAMI_API_KEY", hide_env_values = true)]
        solami_api_key: Option<String>,
        /// Override Solami's gRPC endpoint.
        #[arg(long, env = "SOLAMI_GRPC_URL")]
        grpc_url: Option<String>,
        /// Seconds between finalizer passes.
        #[arg(long, default_value_t = 30)]
        finalize_every: u64,
        /// Seconds between repair passes, which list new signatures from RPC and fetch any missed.
        #[arg(long, default_value_t = 600)]
        repair_every: u64,
        #[arg(long)]
        kill_test: bool,
        /// Kill test: the least seconds live before the cut.
        #[arg(long, default_value_t = 60)]
        warmup: u64,
        /// Kill test: transactions that must arrive live before the cut.
        #[arg(long, default_value_t = 2)]
        min_live: usize,
        /// Kill test: the least seconds the source stays down.
        #[arg(long, default_value_t = 300)]
        offline: u64,
        /// Kill test: transactions RPC must list during the outage before the WebSocket comes back.
        #[arg(long, default_value_t = 2)]
        min_gap: usize,
        /// Kill test: the WebSocket comes back after this many seconds regardless.
        #[arg(long, default_value_t = 3_600)]
        max_offline: u64,
        /// Kill test: seconds to keep streaming after recovery, so the window finalizes.
        #[arg(long, default_value_t = 90)]
        settle: u64,
        #[arg(long, default_value = "data/kill-tests")]
        report_dir: PathBuf,
    },
    /// Serve the JSON API, a live feed of new donations and the web app.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8790")]
        listen: std::net::SocketAddr,
        /// The built web app.
        #[arg(long, default_value = "web/dist")]
        web: PathBuf,
        /// Base URL of the operator's public API, for beneficiary names. Unset turns labels off.
        #[arg(long, env = "RELAYSCOPE_LABELS_URL")]
        labels_url: Option<String>,
    },
    /// Counts and totals from the database.
    Stats {
        /// Also list up to this many decode failures.
        #[arg(long, default_value_t = 10)]
        failures: usize,
    },
    /// Run every reconciliation check on every epoch, store the results in the `checks` table
    /// and print a report. Exits with 1 if any check fails.
    Verify {
        /// Failures to list.
        #[arg(long, default_value_t = 50)]
        show: usize,
    },
    /// Re-decode stored transactions with the current decoder, without fetching.
    Reindex {
        /// Only the transactions the decoder rejected before.
        #[arg(long)]
        failed_only: bool,
    },
    /// Work with the program's on-chain IDL.
    Idl {
        #[command(subcommand)]
        command: IdlCommand,
    },
    /// Fetch a transaction and decode everything the relay did in it.
    Tx {
        signature: String,
        /// Print the decoded transaction as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Save a transaction as a test fixture.
    Fixture {
        signature: String,
        /// File name without extension, such as `donation-native-direct`.
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "fixtures")]
        dir: PathBuf,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum LiveSource {
    Websocket,
    Gapless,
}

#[derive(Subcommand)]
enum IdlCommand {
    /// Download the IDL and pin it, with its SHA-256.
    Fetch {
        #[arg(long, default_value = "idl/donation_relay.json")]
        out: PathBuf,
        /// Fetch another program's Anchor IDL instead, for comparison.
        #[arg(long)]
        program: Option<String>,
    },
    /// Compare the on-chain IDL with the pinned copy. Exits with 1 if they differ.
    Check {
        #[arg(long, default_value = "idl/donation_relay.json")]
        pinned: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // A local .env may hold RELAYSCOPE_RPC_URL and other settings. It is optional.
    dotenvy::dotenv().ok();
    // More than one TLS crypto backend is linked in (gapless and the WebSocket client bring
    // their own), so pick one explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let rpc = Rpc::new(&cli.rpc_url, cli.rps);

    match cli.command {
        Command::Backfill {
            concurrency,
            batch,
            max,
            list_only,
            skip_list,
        } => {
            let mut store = Store::open(&cli.db)?;
            eprintln!(
                "backfill into {} from {} at {} requests/s",
                cli.db.display(),
                rpc.host(),
                cli.rps
            );
            if !skip_list {
                let listed = backfill::list(&rpc, &mut store).await?;
                eprintln!(
                    "listed {} pages, {} new signatures",
                    listed.pages, listed.added
                );
            }
            if !list_only {
                let options = backfill::FetchOptions {
                    concurrency,
                    batch,
                    max,
                };
                let fetched = backfill::fetch(&rpc, &mut store, &options).await?;
                eprintln!(
                    "stored {} transactions, {} decode failures, {} could not be fetched ({} still queued)",
                    fetched.stored,
                    fetched.decode_failures,
                    fetched.fetch_failures,
                    store.count_unfetched()?
                );
                if fetched.decode_failures > 0 {
                    eprintln!("run `relayscope stats` to see the decode failures");
                    std::process::exit(2);
                }
            }
        }
        Command::Live {
            source,
            ws_url,
            solami_api_key,
            grpc_url,
            finalize_every,
            repair_every,
            kill_test,
            warmup,
            min_live,
            offline,
            min_gap,
            max_offline,
            settle,
            report_dir,
        } => {
            let mut store = Store::open(&cli.db)?;
            let source = match source {
                LiveSource::Websocket => live::Source::WebSocket {
                    url: ws_url.unwrap_or_else(|| {
                        cli.rpc_url
                            .replacen("https://", "wss://", 1)
                            .replacen("http://", "ws://", 1)
                    }),
                },
                LiveSource::Gapless => live::Source::Gapless {
                    api_key: solami_api_key.context("--source gapless needs SOLAMI_API_KEY")?,
                    grpc_url,
                },
            };
            let options = live::LiveOptions {
                source,
                finalize_every: std::time::Duration::from_secs(finalize_every),
                repair_every: std::time::Duration::from_secs(repair_every),
                kill_test: kill_test.then(|| live::KillTestOptions {
                    warmup: std::time::Duration::from_secs(warmup),
                    min_live,
                    offline: std::time::Duration::from_secs(offline),
                    min_gap,
                    max_offline: std::time::Duration::from_secs(max_offline),
                    settle: std::time::Duration::from_secs(settle),
                    report_dir,
                }),
            };
            let passed = live::run(&rpc, &mut store, options).await?;
            if !passed {
                std::process::exit(1);
            }
        }
        Command::Serve {
            listen,
            web,
            labels_url,
        } => {
            // Apply any pending migrations before serving read-only.
            drop(Store::open(&cli.db)?);
            server::run(server::ServeOptions {
                listen,
                db: cli.db.clone(),
                web,
                labels_url,
            })
            .await?;
        }
        Command::Stats { failures } => {
            let store = Store::open(&cli.db)?;
            stats::print(&store.stats()?);
            let listed = store.decode_failures(failures)?;
            if !listed.is_empty() {
                println!("\ndecode failures (newest first)");
                for (signature, error) in listed {
                    println!("  {signature}: {error}");
                }
            }
        }
        Command::Verify { show } => {
            let mut store = Store::open(&cli.db)?;
            let report = verify::run(&mut store, show)?;
            if report.failures > 0 {
                std::process::exit(1);
            }
        }
        Command::Reindex { failed_only } => {
            let mut store = Store::open(&cli.db)?;
            let report = store.reindex(failed_only, |raw| {
                Ok(decode_transaction(&TxView::from_rpc_json(raw)?)?)
            })?;
            println!(
                "re-decoded {} transactions, {} failed",
                report.decoded, report.failed
            );
        }
        Command::Idl {
            command: IdlCommand::Fetch { out, program },
        } => {
            let program = match program {
                Some(p) => p
                    .parse()
                    .map_err(|e| anyhow::anyhow!("bad program ID {p}: {e}"))?,
                None => relayscope_core::PROGRAM_ID,
            };
            let bytes = idl::fetch(&rpc, &program).await?;
            let digest = idl::write_pinned(&out, &bytes)?;
            println!(
                "wrote {} ({} bytes), sha256 {digest}",
                out.display(),
                bytes.len()
            );
        }
        Command::Idl {
            command: IdlCommand::Check { pinned },
        } => match idl::check(&rpc, &pinned).await? {
            idl::CheckResult::Matches(digest) => println!(
                "on-chain IDL matches {} (sha256 {digest})",
                pinned.display()
            ),
            idl::CheckResult::Differs { pinned: p, onchain } => {
                eprintln!(
                    "on-chain IDL differs from the pinned copy\n  pinned   {p}\n  on-chain {onchain}"
                );
                std::process::exit(1);
            }
        },
        Command::Tx { signature, json } => {
            let raw = rpc.transaction(&signature).await?;
            let decoded = decode_transaction(&TxView::from_rpc_json(&raw)?)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&decoded)?);
            } else {
                print_summary(&decoded);
            }
        }
        Command::Fixture {
            signature,
            name,
            dir,
        } => {
            let raw = rpc.transaction(&signature).await?;
            // Refuse to save anything the decoder cannot read.
            let decoded = decode_transaction(&TxView::from_rpc_json(&raw)?)?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{name}.json"));
            std::fs::write(&path, serde_json::to_string_pretty(&raw)? + "\n")
                .with_context(|| format!("writing {}", path.display()))?;
            println!(
                "saved {} ({} instructions, {} events)",
                path.display(),
                decoded.instructions.len(),
                decoded.events.len()
            );
        }
    }
    Ok(())
}

fn print_summary(tx: &DecodedTx) {
    let time = tx
        .block_time
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown time".into());
    let status = match (&tx.succeeded, &tx.program_error) {
        (true, _) => "succeeded".to_string(),
        (false, Some(e)) => format!("failed: {} ({})", e.name, e.message),
        (false, None) => "failed".to_string(),
    };
    println!(
        "{}\n  slot {}, {time}, {status}\n  fee payer {}",
        tx.signature, tx.slot, tx.fee_payer
    );

    for (i, ix) in tx.instructions.iter().enumerate() {
        let caller = ix
            .invoked_by
            .map_or("called directly".to_string(), |p| format!("called by {p}"));
        println!(
            "\n  [{i}] {} at {}, {caller}",
            ix.instruction.name(),
            ix.position
        );
        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&ix.instruction)
            && let Some(serde_json::Value::Object(args)) = map.get("args")
        {
            for (k, v) in args {
                println!("      {k}: {}", compact(v));
            }
        }
    }
    for event in &tx.events {
        let from = event
            .emitted_by
            .map_or(String::new(), |i| format!(", from [{i}]"));
        println!(
            "\n  event {} at {}{from}",
            event.event.name(),
            event.position
        );
        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&event.event)
            && let Some(serde_json::Value::Object(fields)) = map.get("fields")
        {
            for (k, v) in fields
                .iter()
                .filter(|(k, _)| *k != "daas_event_discriminator")
            {
                println!("      {k}: {}", compact(v));
            }
        }
    }
    let moved: Vec<_> = tx
        .token_balances
        .iter()
        .filter(|b| b.pre != b.post)
        .collect();
    if !moved.is_empty() {
        println!("\n  token balance changes");
        for b in moved {
            let delta = i128::from(b.post) - i128::from(b.pre);
            println!("      {} {:+} (mint {})", b.account, delta, b.mint);
        }
    }
}

fn compact(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
