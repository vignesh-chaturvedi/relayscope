//! The program's seven events. Each is emitted through Anchor's event CPI, so it appears in a
//! transaction's inner instructions as `EVENT_IX_TAG ++ discriminator ++ borsh(event)`.

use serde::Serialize;

use crate::error::DecodeError;
use crate::program::EVENT_IX_TAG;
use crate::pubkey::{Bytes32, Pubkey};
use crate::shape::{IdlType, Shape, idl_struct};
use crate::types::{DaasEventDiscriminator, ExactTransfer, Receipt};

idl_struct! {
    /// A direct donation, recorded in the `(config_id, mint)` pool for `epoch`.
    pub struct DonationMadeV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub gross_amount: u64,
        pub tip: u64,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// An EVM donation that arrived through the bridge and was admitted into a config's pool.
    pub struct BridgeDonationAdmittedV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub source_id: Bytes32,
        pub source_commitment: Bytes32,
        pub payment_id: Bytes32,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub gross_amount: u64,
        pub tip: u64,
    }
}

idl_struct! {
    /// A bridge payment returned by the upgrade authority instead of being admitted.
    pub struct BridgePaymentRecoveredV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub source_id: Bytes32,
        pub source_commitment: Bytes32,
        pub payment_id: Bytes32,
        pub mint: Pubkey,
        pub receiver_token_account: Pubkey,
        pub destination_owner: Pubkey,
        pub destination_token_account: Pubkey,
        pub amount: u64,
        pub authority: Pubkey,
    }
}

idl_struct! {
    /// An epoch was closed and its pool moved into a distribution account.
    pub struct DonationEpochClosedV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub gross_epoch_amount: u64,
    }
}

idl_struct! {
    /// A closed epoch was swapped into the settlement mint.
    pub struct DonationEpochJustSwappedV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub distribution: Pubkey,
        pub swap_receipt: Pubkey,
        pub output_token_account: Pubkey,
        pub swap_output_amount: u64,
        pub settlement_mint: Pubkey,
    }
}

idl_struct! {
    /// A closed epoch was paid out in exact amounts.
    pub struct DonationEpochProcessedExactV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub settled_amount: u64,
        pub settlement_mint: Pubkey,
        pub transfers: Vec<ExactTransfer>,
    }
}

idl_struct! {
    /// A closed epoch was paid out by basis points (older payouts).
    pub struct DonationEpochProcessedV1Event {
        pub daas_event_discriminator: DaasEventDiscriminator,
        pub config_id: Bytes32,
        pub mint: Pubkey,
        pub epoch: u128,
        pub settled_amount: u64,
        pub settlement_mint: Pubkey,
        pub transfers: Vec<Receipt>,
    }
}

/// Name, discriminator and layout of one event, for the IDL test.
#[derive(Clone, Copy, Debug)]
pub struct EventInfo {
    pub name: &'static str,
    pub discriminator: [u8; 8],
    pub shape: Shape,
}

macro_rules! events {
    ($( $variant:ident = [$($d:literal),*] ),* $(,)?) => {
        /// A decoded event.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(tag = "name", content = "fields")]
        pub enum Event {
            $( $variant($variant) ),*
        }

        /// Every event the decoder knows, in IDL order.
        pub const EVENTS: &[EventInfo] = &[
            $( EventInfo {
                name: stringify!($variant),
                discriminator: [$($d),*],
                shape: <$variant as IdlType>::SHAPE,
            } ),*
        ];

        impl Event {
            /// Decodes an event from the data of a self-CPI instruction, tag included.
            pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
                let body = data.strip_prefix(&EVENT_IX_TAG[..]).ok_or(DecodeError::NotAnEvent)?;
                let (discriminator, fields) = split_discriminator(body)?;
                $(
                    if discriminator == [$($d),*] {
                        return borsh::from_slice(fields)
                            .map(Self::$variant)
                            .map_err(|source| DecodeError::Borsh { what: stringify!($variant), source });
                    }
                )*
                Err(DecodeError::UnknownEvent(discriminator))
            }

            pub fn name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => stringify!($variant) ),*
                }
            }
        }
    };
}

events! {
    BridgeDonationAdmittedV1Event = [177, 96, 71, 84, 43, 156, 177, 164],
    BridgePaymentRecoveredV1Event = [88, 244, 161, 211, 183, 143, 72, 32],
    DonationEpochClosedV1Event = [155, 169, 23, 14, 6, 117, 86, 147],
    DonationEpochJustSwappedV1Event = [200, 216, 187, 37, 35, 206, 206, 127],
    DonationEpochProcessedExactV1Event = [243, 73, 72, 69, 143, 14, 76, 204],
    DonationEpochProcessedV1Event = [225, 251, 131, 171, 232, 182, 67, 59],
    DonationMadeV1Event = [165, 229, 86, 237, 244, 77, 38, 222],
}

impl Event {
    /// Whether instruction data is an event CPI.
    pub fn is_event_data(data: &[u8]) -> bool {
        data.starts_with(&EVENT_IX_TAG)
    }

    /// The `(config_id, mint, epoch)` pool this event belongs to, if it names one.
    pub fn pool(&self) -> Option<(Bytes32, Pubkey, u128)> {
        match self {
            Self::DonationMadeV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::BridgeDonationAdmittedV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::DonationEpochClosedV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::DonationEpochJustSwappedV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::DonationEpochProcessedExactV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::DonationEpochProcessedV1Event(e) => Some((e.config_id, e.mint, e.epoch)),
            Self::BridgePaymentRecoveredV1Event(_) => None,
        }
    }
}

pub(crate) fn split_discriminator(data: &[u8]) -> Result<([u8; 8], &[u8]), DecodeError> {
    if data.len() < 8 {
        return Err(DecodeError::TooShort(data.len()));
    }
    let (head, rest) = data.split_at(8);
    Ok((head.try_into().expect("split at 8"), rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_payment_round_trips() {
        // `recover_bridge_payment_v1` has never run on mainnet, so this event has no real
        // sample. A round trip proves the decoder reads what the layout writes.
        let event = BridgePaymentRecoveredV1Event {
            daas_event_discriminator: DaasEventDiscriminator::BridgePaymentRecoveredV1,
            source_id: Bytes32([1; 32]),
            source_commitment: Bytes32([2; 32]),
            payment_id: Bytes32([3; 32]),
            mint: Pubkey([4; 32]),
            receiver_token_account: Pubkey([5; 32]),
            destination_owner: Pubkey([6; 32]),
            destination_token_account: Pubkey([7; 32]),
            amount: 1_234_567,
            authority: Pubkey([8; 32]),
        };
        let mut data = EVENT_IX_TAG.to_vec();
        data.extend_from_slice(&[88, 244, 161, 211, 183, 143, 72, 32]);
        data.extend_from_slice(&borsh::to_vec(&event).unwrap());
        assert_eq!(
            Event::decode(&data).unwrap(),
            Event::BridgePaymentRecoveredV1Event(event)
        );
    }

    #[test]
    fn unknown_discriminator_is_an_error() {
        let mut data = EVENT_IX_TAG.to_vec();
        data.extend_from_slice(&[0; 8]);
        assert!(matches!(
            Event::decode(&data),
            Err(DecodeError::UnknownEvent(_))
        ));
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let event = DonationEpochClosedV1Event {
            daas_event_discriminator: DaasEventDiscriminator::DonationEpochClosedV1,
            config_id: Bytes32([9; 32]),
            mint: Pubkey([1; 32]),
            epoch: 3,
            gross_epoch_amount: 10,
        };
        let mut data = EVENT_IX_TAG.to_vec();
        data.extend_from_slice(&[155, 169, 23, 14, 6, 117, 86, 147]);
        data.extend_from_slice(&borsh::to_vec(&event).unwrap());
        data.push(0);
        assert!(matches!(
            Event::decode(&data),
            Err(DecodeError::Borsh { .. })
        ));
    }
}
