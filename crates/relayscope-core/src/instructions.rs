//! The program's fifteen instructions: their arguments and the names of their accounts.

use serde::Serialize;

use crate::error::DecodeError;
use crate::events::split_discriminator;
use crate::pubkey::{Bytes, Bytes32, Pubkey};
use crate::shape::{IdlType, Shape, idl_struct};
use crate::types::{ExactTransfer, Share};

idl_struct! {
    /// Admits a bridged EVM payment into a config's USDC pool (relayer only).
    pub struct AdmitBridgePaymentV1 {
        pub source_id: Bytes32,
        pub source_commitment: Bytes32,
        pub payment_id: Bytes32,
        pub config_id: Bytes32,
        pub amount: u64,
        pub minimum_amount: u64,
        pub tip_bps: u16,
    }
}

idl_struct! {
    /// Closes an epoch and moves its pool into a distribution account (relayer only).
    pub struct CloseDonationEpochV1 {
        pub config_id: Bytes32,
        pub epoch: u128,
    }
}

idl_struct! {
    /// Older payout after a swap, by basis points (relayer only).
    pub struct DistributeAfterJustSwappingV1 {
        pub shares: Vec<Share>,
    }
}

idl_struct! {
    /// Current payout after a swap, in exact amounts (relayer only).
    pub struct DistributeExactAfterJustSwappingV1 {
        pub transfers: Vec<ExactTransfer>,
    }
}

idl_struct! {
    /// Current payout of an epoch already in the settlement mint, in exact amounts (relayer only).
    pub struct DistributeExactNonSwapOutputMintV1 {
        pub transfers: Vec<ExactTransfer>,
    }
}

idl_struct! {
    /// Older payout of an epoch already in the settlement mint, by basis points (relayer only).
    pub struct DistributeNonSwapOutputMintV1 {
        pub shares: Vec<Share>,
    }
}

idl_struct! {
    /// Donates native SOL, with the config ID as a pubkey.
    pub struct DonateNativePubkeyConfigIdV1 {
        pub amount: u64,
        pub config_id: Pubkey,
        pub tip_bps: u16,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// Donates native SOL, with the config ID as raw bytes.
    pub struct DonateNativeV1 {
        pub amount: u64,
        pub config_id: Bytes32,
        pub tip_bps: u16,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// Donates an SPL or Token-2022 token, with the config ID as a pubkey.
    pub struct DonatePubkeyConfigIdV1 {
        pub amount: u64,
        pub config_id: Pubkey,
        pub tip_bps: u16,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// Donates a token from an account that cannot pay rent, such as a PDA escrow, with a separate payer.
    pub struct DonatePubkeyConfigIdWithPayerV1 {
        pub amount: u64,
        pub config_id: Pubkey,
        pub tip_bps: u16,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// Donates an SPL or Token-2022 token, with the config ID as raw bytes.
    pub struct DonateV1 {
        pub amount: u64,
        pub config_id: Bytes32,
        pub tip_bps: u16,
        pub message: String,
        pub credited_to: Pubkey,
    }
}

idl_struct! {
    /// Swaps a closed epoch through Jupiter (relayer only).
    pub struct JustSwapV1 {
        pub jupiter_swap_ix_data: Bytes,
    }
}

idl_struct! {
    /// Returns a bridge payment instead of admitting it (upgrade authority only).
    pub struct RecoverBridgePaymentV1 {
        pub source_id: Bytes32,
        pub source_commitment: Bytes32,
        pub payment_id: Bytes32,
        pub amount: u64,
    }
}

idl_struct! {
    /// Older combined swap and bps payout (relayer only).
    pub struct SwapThenDistributeV1 {
        pub jupiter_swap_ix_data: Bytes,
        pub shares: Vec<Share>,
    }
}

idl_struct! {
    /// Adds mints to the donation whitelist (relayer only).
    pub struct UpsertMintWhitelistV1 {
        pub mints: Vec<Pubkey>,
    }
}

/// Name, discriminator, argument layout and account names of one instruction.
#[derive(Clone, Copy, Debug)]
pub struct InstructionInfo {
    pub name: &'static str,
    pub discriminator: [u8; 8],
    pub args: Shape,
    pub accounts: &'static [&'static str],
}

macro_rules! instructions {
    ($( $variant:ident = $name:literal, [$($d:literal),*], [$($account:literal),*] );* $(;)?) => {
        /// A decoded instruction and its arguments.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(tag = "name", content = "args", rename_all = "snake_case")]
        pub enum Instruction {
            $( $variant($variant) ),*
        }

        /// Every instruction the decoder knows, in IDL order.
        pub const INSTRUCTIONS: &[InstructionInfo] = &[
            $( InstructionInfo {
                name: $name,
                discriminator: [$($d),*],
                args: <$variant as IdlType>::SHAPE,
                accounts: &[$($account),*],
            } ),*
        ];

        impl Instruction {
            pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
                let (discriminator, args) = split_discriminator(data)?;
                $(
                    if discriminator == [$($d),*] {
                        return borsh::from_slice(args)
                            .map(Self::$variant)
                            .map_err(|source| DecodeError::Borsh { what: $name, source });
                    }
                )*
                Err(DecodeError::UnknownInstruction(discriminator))
            }

            pub fn name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $name ),*
                }
            }

            /// The IDL's names for this instruction's accounts, in order.
            pub fn account_names(&self) -> &'static [&'static str] {
                match self {
                    $( Self::$variant(_) => &[$($account),*] ),*
                }
            }
        }
    };
}

instructions! {
    AdmitBridgePaymentV1 = "admit_bridge_payment_v1", [60, 91, 185, 5, 133, 97, 225, 55],
        ["source_marker", "payment_marker", "receiver", "receiver_token_account", "epoch_tracker",
         "debouncer", "debouncer_token_account", "mint", "signer", "token_program",
         "associated_token_program", "system_program", "event_authority", "program"];
    CloseDonationEpochV1 = "close_donation_epoch_v1", [171, 28, 54, 214, 140, 137, 101, 90],
        ["distribution", "distribution_token_account", "epoch_tracker", "debouncer",
         "debouncer_token_account", "mint", "token_program", "signer", "associated_token_program",
         "system_program", "event_authority", "program"];
    DistributeAfterJustSwappingV1 = "distribute_after_just_swapping_v1", [210, 176, 49, 190, 147, 105, 115, 119],
        ["epoch_tracker", "debouncer", "distribution", "swap_receipt",
         "output_distribution_token_account", "output_mint", "output_mint_program", "signer",
         "associated_token_program", "system_program", "event_authority", "program"];
    DistributeExactAfterJustSwappingV1 = "distribute_exact_after_just_swapping_v1", [125, 141, 74, 222, 144, 50, 31, 201],
        ["epoch_tracker", "debouncer", "distribution", "swap_receipt",
         "output_distribution_token_account", "output_mint", "output_mint_program", "signer",
         "associated_token_program", "system_program", "event_authority", "program"];
    DistributeExactNonSwapOutputMintV1 = "distribute_exact_non_swap_output_mint_v1", [142, 145, 85, 8, 14, 20, 4, 205],
        ["epoch_tracker", "debouncer", "distribution", "distribution_token_account", "mint",
         "signer", "token_program", "associated_token_program", "system_program",
         "event_authority", "program"];
    DistributeNonSwapOutputMintV1 = "distribute_non_swap_output_mint_v1", [152, 149, 100, 48, 118, 236, 9, 243],
        ["epoch_tracker", "debouncer", "distribution", "distribution_token_account", "mint",
         "signer", "token_program", "associated_token_program", "system_program",
         "event_authority", "program"];
    DonateNativePubkeyConfigIdV1 = "donate_native_pubkey_config_id_v1", [192, 254, 132, 16, 208, 43, 154, 82],
        ["epoch_tracker", "debouncer", "debouncer_token_account", "mint", "temp_wsol_token_account",
         "from", "mint_whitelist", "associated_token_program", "token_program", "system_program",
         "event_authority", "program"];
    DonateNativeV1 = "donate_native_v1", [121, 160, 15, 44, 219, 43, 134, 7],
        ["epoch_tracker", "debouncer", "debouncer_token_account", "mint", "temp_wsol_token_account",
         "from", "mint_whitelist", "associated_token_program", "token_program", "system_program",
         "event_authority", "program"];
    DonatePubkeyConfigIdV1 = "donate_pubkey_config_id_v1", [187, 141, 207, 216, 60, 246, 196, 155],
        ["epoch_tracker", "debouncer", "debouncer_token_account", "mint", "from_token_account",
         "from", "mint_whitelist", "associated_token_program", "token_program", "system_program",
         "event_authority", "program"];
    DonatePubkeyConfigIdWithPayerV1 = "donate_pubkey_config_id_with_payer_v1", [120, 217, 57, 241, 135, 104, 139, 184],
        ["epoch_tracker", "debouncer", "debouncer_token_account", "mint", "from_token_account",
         "from", "payer", "mint_whitelist", "associated_token_program", "token_program",
         "system_program", "event_authority", "program"];
    DonateV1 = "donate_v1", [136, 8, 102, 156, 214, 4, 17, 171],
        ["epoch_tracker", "debouncer", "debouncer_token_account", "mint", "from_token_account",
         "from", "mint_whitelist", "associated_token_program", "token_program", "system_program",
         "event_authority", "program"];
    JustSwapV1 = "just_swap_v1", [204, 159, 132, 117, 132, 86, 223, 73],
        ["epoch_tracker", "debouncer", "distribution", "swap_receipt", "distribution_token_account",
         "output_distribution_token_account", "mint", "output_mint", "output_mint_program", "signer",
         "token_program", "jupiter_program", "associated_token_program", "system_program",
         "event_authority", "program"];
    RecoverBridgePaymentV1 = "recover_bridge_payment_v1", [254, 136, 225, 245, 220, 56, 79, 62],
        ["source_marker", "payment_marker", "receiver", "receiver_token_account", "destination_owner",
         "destination_token_account", "mint", "program_data", "authority", "payer", "token_program",
         "associated_token_program", "system_program", "event_authority", "program"];
    SwapThenDistributeV1 = "swap_then_distribute_v1", [218, 85, 237, 178, 24, 174, 102, 41],
        ["epoch_tracker", "debouncer", "distribution", "distribution_token_account",
         "output_distribution_token_account", "mint", "output_mint", "output_mint_program", "signer",
         "token_program", "jupiter_program", "associated_token_program", "system_program",
         "event_authority", "program"];
    UpsertMintWhitelistV1 = "upsert_mint_whitelist_v1", [209, 133, 253, 12, 80, 9, 231, 128],
        ["mint_whitelist", "signer", "system_program"];
}

impl Instruction {
    /// The config ID for instructions that carry one, normalised to 32 bytes.
    pub fn config_id(&self) -> Option<Bytes32> {
        match self {
            Self::AdmitBridgePaymentV1(ix) => Some(ix.config_id),
            Self::CloseDonationEpochV1(ix) => Some(ix.config_id),
            Self::DonateNativePubkeyConfigIdV1(ix) => Some(ix.config_id.into()),
            Self::DonateNativeV1(ix) => Some(ix.config_id),
            Self::DonatePubkeyConfigIdV1(ix) => Some(ix.config_id.into()),
            Self::DonatePubkeyConfigIdWithPayerV1(ix) => Some(ix.config_id.into()),
            Self::DonateV1(ix) => Some(ix.config_id),
            _ => None,
        }
    }

    /// Whether this is one of the five public donate instructions.
    pub fn is_donation(&self) -> bool {
        matches!(
            self,
            Self::DonateNativePubkeyConfigIdV1(_)
                | Self::DonateNativeV1(_)
                | Self::DonatePubkeyConfigIdV1(_)
                | Self::DonatePubkeyConfigIdWithPayerV1(_)
                | Self::DonateV1(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_bridge_payment_round_trips() {
        // Never run on mainnet, so there is no real sample to test against.
        let args = RecoverBridgePaymentV1 {
            source_id: Bytes32([1; 32]),
            source_commitment: Bytes32([2; 32]),
            payment_id: Bytes32([3; 32]),
            amount: 42,
        };
        let mut data = vec![254, 136, 225, 245, 220, 56, 79, 62];
        data.extend_from_slice(&borsh::to_vec(&args).unwrap());
        let ix = Instruction::decode(&data).unwrap();
        assert_eq!(ix.name(), "recover_bridge_payment_v1");
        assert_eq!(ix, Instruction::RecoverBridgePaymentV1(args));
    }

    #[test]
    fn config_id_is_the_same_bytes_in_both_encodings() {
        let raw = DonateV1 {
            amount: 1,
            config_id: Bytes32([7; 32]),
            tip_bps: 0,
            message: String::new(),
            credited_to: Pubkey([0; 32]),
        };
        let keyed = DonatePubkeyConfigIdV1 {
            amount: 1,
            config_id: Pubkey([7; 32]),
            tip_bps: 0,
            message: String::new(),
            credited_to: Pubkey([0; 32]),
        };
        assert_eq!(
            Instruction::DonateV1(raw).config_id(),
            Instruction::DonatePubkeyConfigIdV1(keyed).config_id()
        );
    }
}

#[cfg(test)]
mod unseen_on_mainnet {
    //! `donate_native_v1` and `donate_pubkey_config_id_v1` did not appear in a sample of 562
    //! transactions spread across the program's history. Round trips cover them until a real
    //! sample turns up in the Phase 2 backfill.
    use super::*;

    fn round_trip(discriminator: [u8; 8], args: &impl borsh::BorshSerialize) -> Instruction {
        let mut data = discriminator.to_vec();
        data.extend_from_slice(&borsh::to_vec(args).unwrap());
        Instruction::decode(&data).unwrap()
    }

    #[test]
    fn donate_native_v1() {
        let args = DonateNativeV1 {
            amount: 5,
            config_id: Bytes32([1; 32]),
            tip_bps: 250,
            message: "gm".into(),
            credited_to: Pubkey([2; 32]),
        };
        assert_eq!(
            round_trip([121, 160, 15, 44, 219, 43, 134, 7], &args),
            Instruction::DonateNativeV1(args)
        );
    }

    #[test]
    fn donate_pubkey_config_id_v1() {
        let args = DonatePubkeyConfigIdV1 {
            amount: 5,
            config_id: Pubkey([1; 32]),
            tip_bps: 0,
            message: String::new(),
            credited_to: Pubkey([2; 32]),
        };
        assert_eq!(
            round_trip([187, 141, 207, 216, 60, 246, 196, 155], &args),
            Instruction::DonatePubkeyConfigIdV1(args)
        );
    }
}
