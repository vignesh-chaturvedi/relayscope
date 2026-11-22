//! Fetches the program's IDL from its Anchor IDL account and compares it with the pinned copy.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use relayscope_core::{PROGRAM_ID, pda};
use sha2::{Digest, Sha256};

use crate::rpc::Rpc;

/// Anchor's IDL account: 8-byte discriminator, 32-byte authority, u32 length, zlib data.
const HEADER: usize = 8 + 32 + 4;

/// Downloads and decompresses the on-chain IDL, byte for byte.
pub async fn fetch(rpc: &Rpc) -> Result<Vec<u8>> {
    let address = pda::idl_address(&PROGRAM_ID);
    let data = rpc
        .account_data(&address.to_string())
        .await?
        .with_context(|| format!("IDL account {address} does not exist"))?;
    if data.len() < HEADER {
        bail!("IDL account {address} is too small: {} bytes", data.len());
    }
    let len = u32::from_le_bytes(data[40..44].try_into().expect("4 bytes")) as usize;
    let compressed = data
        .get(HEADER..HEADER + len)
        .with_context(|| format!("IDL account declares {len} bytes but holds fewer"))?;
    let mut json = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut json)
        .context("IDL data is not valid zlib")?;
    Ok(json)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Writes the IDL and its `sha256sum`-style digest file next to it.
pub fn write_pinned(path: &Path, idl: &[u8]) -> Result<String> {
    let digest = sha256_hex(idl);
    std::fs::write(path, idl).with_context(|| format!("writing {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("donation_relay.json");
    let digest_path = path.with_extension("json.sha256");
    std::fs::write(&digest_path, format!("{digest}  {name}\n"))
        .with_context(|| format!("writing {}", digest_path.display()))?;
    Ok(digest)
}

pub enum CheckResult {
    Matches(String),
    Differs { pinned: String, onchain: String },
}

/// Compares the on-chain IDL with the pinned file.
pub async fn check(rpc: &Rpc, pinned: &Path) -> Result<CheckResult> {
    let pinned_bytes =
        std::fs::read(pinned).with_context(|| format!("reading {}", pinned.display()))?;
    let pinned_digest = sha256_hex(&pinned_bytes);
    let recorded =
        std::fs::read_to_string(pinned.with_extension("json.sha256")).unwrap_or_default();
    if !recorded.starts_with(&pinned_digest) {
        bail!(
            "{} does not match its .sha256 file; re-pin it with `relayscope idl fetch`",
            pinned.display()
        );
    }
    let onchain_digest = sha256_hex(&fetch(rpc).await?);
    Ok(if onchain_digest == pinned_digest {
        CheckResult::Matches(pinned_digest)
    } else {
        CheckResult::Differs {
            pinned: pinned_digest,
            onchain: onchain_digest,
        }
    })
}
