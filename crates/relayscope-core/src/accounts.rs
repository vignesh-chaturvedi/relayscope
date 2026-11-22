//! The program's seven account types.

use borsh::BorshDeserialize;
use serde::Serialize;

use crate::error::DecodeError;
use crate::events::split_discriminator;
use crate::pubkey::{Bytes32, Pubkey};
use crate::shape::{IdlType, Shape, idl_struct};
use crate::types::{DebouncerV1State, DistributionV1State, EpochTrackerV1State};

idl_struct! {
    /// Marks a bridge payment as admitted. It has no fields; its existence is the record.
    pub struct BridgePaymentV1 {}
}

idl_struct! {
    /// Marks a bridge source as used. It has no fields; its existence is the record.
    pub struct BridgeSourceV1 {}
}

idl_struct! {
    /// The vault authority and running total for a `(config_id, mint)` pool.
    pub struct DebouncerV1 {
        pub bump: u8,
        pub state: DebouncerV1State,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub total_amount: u64,
    }
}

idl_struct! {
    /// Holds one closed epoch until it is paid out.
    pub struct DistributionV1 {
        pub bump: u8,
        pub state: DistributionV1State,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub total_amount: u64,
    }
}

idl_struct! {
    /// The current open epoch for a `(config_id, mint)` pool.
    pub struct EpochTrackerV1 {
        pub bump: u8,
        pub state: EpochTrackerV1State,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub current_epoch: u128,
    }
}

idl_struct! {
    /// Mints allowed to skip the strict mint safety check.
    pub struct MintWhitelistV1 {
        pub bump: u8,
        pub mints: Vec<Pubkey>,
    }
}

idl_struct! {
    /// Records a swap's output for the payout that follows it.
    pub struct SwapReceiptV1 {
        pub bump: u8,
        pub distribution: Pubkey,
        pub swap_output_amount: u64,
    }
}

/// Name, discriminator and layout of one account type, for the IDL test.
#[derive(Clone, Copy, Debug)]
pub struct AccountInfo {
    pub name: &'static str,
    pub discriminator: [u8; 8],
    pub shape: Shape,
}

macro_rules! accounts {
    ($( $variant:ident = [$($d:literal),*] ),* $(,)?) => {
        /// A decoded program account.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(tag = "name", content = "fields")]
        pub enum Account {
            $( $variant($variant) ),*
        }

        /// Every account type the decoder knows, in IDL order.
        pub const ACCOUNTS: &[AccountInfo] = &[
            $( AccountInfo {
                name: stringify!($variant),
                discriminator: [$($d),*],
                shape: <$variant as IdlType>::SHAPE,
            } ),*
        ];

        impl Account {
            /// Decodes account data. Accounts can be larger than their fields, so trailing
            /// bytes are allowed here, unlike for instructions and events.
            pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
                let (discriminator, mut fields) = split_discriminator(data)?;
                $(
                    if discriminator == [$($d),*] {
                        return $variant::deserialize(&mut fields)
                            .map(Self::$variant)
                            .map_err(|source| DecodeError::Borsh { what: stringify!($variant), source });
                    }
                )*
                Err(DecodeError::UnknownAccount(discriminator))
            }

            pub fn name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => stringify!($variant) ),*
                }
            }
        }
    };
}

accounts! {
    BridgePaymentV1 = [12, 3, 97, 154, 192, 230, 153, 50],
    BridgeSourceV1 = [7, 145, 128, 53, 99, 140, 98, 249],
    DebouncerV1 = [137, 137, 192, 77, 110, 184, 189, 28],
    DistributionV1 = [228, 175, 114, 137, 165, 18, 11, 14],
    EpochTrackerV1 = [130, 12, 50, 226, 170, 109, 160, 155],
    MintWhitelistV1 = [73, 115, 177, 235, 234, 1, 95, 67],
    SwapReceiptV1 = [230, 238, 44, 28, 252, 27, 66, 222],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_debouncer_with_its_exact_space() {
        // The program's documentation gives DebouncerV1 82 bytes of space: 8 + 1 + 1 + 32 + 32 + 8.
        let account = DebouncerV1 {
            bump: 254,
            state: DebouncerV1State::Initialized,
            config_id: Bytes32([3; 32]),
            mint: Pubkey([4; 32]),
            total_amount: 99,
        };
        let mut data = vec![137, 137, 192, 77, 110, 184, 189, 28];
        data.extend_from_slice(&borsh::to_vec(&account).unwrap());
        assert_eq!(data.len(), 82);
        assert_eq!(
            Account::decode(&data).unwrap(),
            Account::DebouncerV1(account)
        );
    }
}
