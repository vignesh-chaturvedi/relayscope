//! Address and fixed-size byte types used throughout the program's IDL.

use std::fmt;
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Serialize, Serializer};

/// A 32-byte Solana address, shown in base58.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&bs58::encode(self.0).into_string())
    }
}

impl fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pubkey({self})")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseKeyError {
    #[error("not valid base58")]
    Base58,
    #[error("not valid hex")]
    Hex,
    #[error("expected 32 bytes, got {0}")]
    Length(usize),
}

impl FromStr for Pubkey {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|_| ParseKeyError::Base58)?;
        let len = bytes.len();
        let array: [u8; 32] = bytes.try_into().map_err(|_| ParseKeyError::Length(len))?;
        Ok(Self(array))
    }
}

impl Serialize for Pubkey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// 32 opaque bytes: config IDs and bridge source, commitment and payment IDs.
///
/// Displayed as `0x`-prefixed hex, the format the operator's public API uses for config IDs. The same
/// bytes read as a [`Pubkey`] give the base58 form.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub struct Bytes32(pub [u8; 32]);

impl Bytes32 {
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.0))
    }

    pub fn to_base58(&self) -> String {
        bs58::encode(self.0).into_string()
    }
}

impl From<Pubkey> for Bytes32 {
    fn from(key: Pubkey) -> Self {
        Self(key.0)
    }
}

impl fmt::Display for Bytes32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Bytes32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes32({})", self.to_hex())
    }
}

impl FromStr for Bytes32 {
    type Err = ParseKeyError;

    /// Accepts `0x`-prefixed hex or base58.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = match s.strip_prefix("0x") {
            Some(hex_str) => hex::decode(hex_str).map_err(|_| ParseKeyError::Hex)?,
            None => bs58::decode(s)
                .into_vec()
                .map_err(|_| ParseKeyError::Base58)?,
        };
        let len = bytes.len();
        let array: [u8; 32] = bytes.try_into().map_err(|_| ParseKeyError::Length(len))?;
        Ok(Self(array))
    }
}

impl Serialize for Bytes32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A variable-length byte string (the IDL's `bytes`), shown as hex.
#[derive(Clone, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes(0x{})", hex::encode(&self.0))
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("0x{}", hex::encode(&self.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_round_trips_through_base58() {
        let s = "RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu";
        let key: Pubkey = s.parse().unwrap();
        assert_eq!(key.to_string(), s);
    }

    #[test]
    fn pubkey_rejects_wrong_length() {
        assert_eq!("11111".parse::<Pubkey>(), Err(ParseKeyError::Length(5)));
    }

    #[test]
    fn bytes32_accepts_hex_and_base58_for_the_same_id() {
        // A real config ID, as the operator's public API returns it in both encodings.
        let hex: Bytes32 = "0x4f015a99ba46d4744bc6e8f51858db96db0e48ef2a54efa77c62ddce968d232d"
            .parse()
            .unwrap();
        let b58: Bytes32 = "6KQQXC52CHfiWmUZYbv8mhwHF7VivW9mMgXfdEFtbwAC"
            .parse()
            .unwrap();
        assert_eq!(hex, b58);
        assert_eq!(
            hex.to_base58(),
            "6KQQXC52CHfiWmUZYbv8mhwHF7VivW9mMgXfdEFtbwAC"
        );
    }
}
