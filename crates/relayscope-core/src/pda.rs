//! Program-derived addresses for the Donation Relay program, and the Anchor IDL account.

use curve25519_dalek::edwards::CompressedEdwardsY;
use sha2::{Digest, Sha256};

use crate::program::PROGRAM_ID;
use crate::pubkey::{Bytes32, Pubkey};

const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";
const MAX_SEEDS: usize = 16;
const MAX_SEED_LEN: usize = 32;

/// Whether 32 bytes decode to a point on the ed25519 curve. A PDA must not.
pub fn is_on_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_some()
}

/// Solana's `create_program_address`: `None` if the seeds are invalid or the hash is on the curve.
pub fn create_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Option<Pubkey> {
    if seeds.len() > MAX_SEEDS || seeds.iter().any(|seed| seed.len() > MAX_SEED_LEN) {
        return None;
    }
    let mut hasher = Sha256::new();
    for seed in seeds {
        hasher.update(seed);
    }
    hasher.update(program_id.as_bytes());
    hasher.update(PDA_MARKER);
    let hash: [u8; 32] = hasher.finalize().into();
    (!is_on_curve(&hash)).then_some(Pubkey(hash))
}

/// Solana's `find_program_address`: the first off-curve address, trying bumps from 255 down.
pub fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Option<(Pubkey, u8)> {
    if seeds.len() >= MAX_SEEDS {
        return None;
    }
    (0..=u8::MAX).rev().find_map(|bump| {
        let bump_seed = [bump];
        let mut with_bump: Vec<&[u8]> = seeds.to_vec();
        with_bump.push(&bump_seed);
        create_program_address(&with_bump, program_id).map(|key| (key, bump))
    })
}

/// Solana's `create_with_seed`.
pub fn create_with_seed(base: &Pubkey, seed: &str, owner: &Pubkey) -> Pubkey {
    let mut hasher = Sha256::new();
    hasher.update(base.as_bytes());
    hasher.update(seed.as_bytes());
    hasher.update(owner.as_bytes());
    Pubkey(hasher.finalize().into())
}

/// The account where Anchor stores a program's compressed IDL.
pub fn idl_address(program_id: &Pubkey) -> Pubkey {
    let (base, _) =
        find_program_address(&[], program_id).expect("an empty seed list always derives");
    create_with_seed(&base, "anchor:idl", program_id)
}

fn derive(seeds: &[&[u8]]) -> Pubkey {
    find_program_address(seeds, &PROGRAM_ID)
        .expect("relay seeds are valid")
        .0
}

/// `epoch_tracker_v1` for a `(config_id, mint)` pair.
pub fn epoch_tracker(config_id: &Bytes32, mint: &Pubkey) -> Pubkey {
    derive(&[b"epoch_tracker_v1", &config_id.0, mint.as_bytes()])
}

/// `debouncer_v1`, the vault authority for a `(config_id, mint)` pair.
pub fn debouncer(config_id: &Bytes32, mint: &Pubkey) -> Pubkey {
    derive(&[b"debouncer_v1", &config_id.0, mint.as_bytes()])
}

/// `distribution_v1` for one closed epoch.
pub fn distribution(config_id: &Bytes32, mint: &Pubkey, epoch: u128) -> Pubkey {
    derive(&[
        b"distribution_v1",
        &config_id.0,
        mint.as_bytes(),
        &epoch.to_le_bytes(),
    ])
}

/// The global `mint_whitelist_v1` account.
pub fn mint_whitelist() -> Pubkey {
    derive(&[b"mint_whitelist_v1"])
}

/// Anchor's event authority, the signer of every event CPI.
pub fn event_authority() -> Pubkey {
    derive(&[b"__event_authority"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idl_address_matches_mainnet() {
        assert_eq!(
            idl_address(&PROGRAM_ID).to_string(),
            "6mCmANHqUcqWCLAUtB3DFJSDv14yWPq4wXUzDhdMj3Ge"
        );
    }

    #[test]
    fn mint_whitelist_matches_mainnet() {
        // Found on mainnet with getProgramAccounts, filtered by the MintWhitelistV1 discriminator.
        assert_eq!(
            mint_whitelist().to_string(),
            "E8R1dqUAhazSpQMjLCEXdC7M7BWgk1vdwaobazKavhoE"
        );
    }

    #[test]
    fn rejects_too_many_and_too_long_seeds() {
        let long = [0u8; 33];
        assert!(create_program_address(&[&long], &PROGRAM_ID).is_none());
        let many: Vec<&[u8]> = vec![b"a"; 17];
        assert!(create_program_address(&many, &PROGRAM_ID).is_none());
    }
}
