mod idl;
mod rpc;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use relayscope_core::{DecodedTx, TxView, decode_transaction};

use crate::rpc::{DEFAULT_RPC_URL, Rpc};

/// Real-time tracker for the Donation Relay program on Solana.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Solana JSON-RPC endpoint.
    #[arg(long, global = true, env = "RELAYSCOPE_RPC_URL", default_value = DEFAULT_RPC_URL)]
    rpc_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    },
    /// Compare the on-chain IDL with the pinned copy. Exits with 1 if they differ.
    Check {
        #[arg(long, default_value = "idl/donation_relay.json")]
        pinned: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(&cli.rpc_url);

    match cli.command {
        Command::Idl {
            command: IdlCommand::Fetch { out },
        } => {
            let bytes = idl::fetch(&rpc).await?;
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
