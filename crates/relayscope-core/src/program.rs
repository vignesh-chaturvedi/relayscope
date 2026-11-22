//! Constants of the deployed Donation Relay program.

use crate::pubkey::Pubkey;

/// `RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu`
pub const PROGRAM_ID: Pubkey = Pubkey::new([
    6, 59, 155, 215, 137, 251, 142, 253, 75, 95, 199, 87, 19, 27, 50, 90, 226, 227, 134, 213, 73,
    162, 89, 217, 195, 168, 135, 154, 54, 85, 230, 148,
]);

/// The first eight bytes of every Anchor event CPI: Anchor's `EVENT_IX_TAG`, the u64
/// `0x1d9acb512ea545e4` (the first eight bytes of `sha256("anchor:event")`), written little-endian.
pub const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

/// The first eight bytes of Anchor's built-in IDL management instructions (`IdlCreateAccount`,
/// `IdlWrite`, `IdlSetBuffer` and so on): Anchor's `IDL_IX_TAG`, the u64 `0x0a69e9a778bcf440`
/// (the first eight bytes of `sha256("anchor:idl")`), written little-endian. They manage the
/// IDL account and never touch donations.
pub const IDL_IX_TAG: [u8; 8] = [0x40, 0xf4, 0xbc, 0x78, 0xa7, 0xe9, 0x69, 0x0a];

/// The IDL version the decoders in this crate were written against.
pub const IDL_VERSION: &str = "0.6.0";

/// SHA-256 of the pinned IDL file, `idl/donation_relay.json`.
pub const IDL_SHA256: &str = include_str!("../../../idl/donation_relay.json.sha256");

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn program_id_is_the_mainnet_address() {
        assert_eq!(
            PROGRAM_ID.to_string(),
            "RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu"
        );
    }

    #[test]
    fn event_tag_is_anchors() {
        let hash = Sha256::digest(b"anchor:event");
        let tag = u64::from_be_bytes(hash[..8].try_into().unwrap());
        assert_eq!(tag, 0x1d9a_cb51_2ea5_45e4);
        assert_eq!(EVENT_IX_TAG, tag.to_le_bytes());
    }

    #[test]
    fn idl_tag_is_anchors() {
        let hash = Sha256::digest(b"anchor:idl");
        let tag = u64::from_be_bytes(hash[..8].try_into().unwrap());
        assert_eq!(tag, 0x0a69_e9a7_78bc_f440);
        assert_eq!(IDL_IX_TAG, tag.to_le_bytes());
    }
}
