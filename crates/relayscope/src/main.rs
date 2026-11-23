mod backfill;
mod idl;
mod rpc;
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
