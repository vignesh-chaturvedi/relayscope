//! Decoding errors, and the program's own error codes.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("data too short for a discriminator: {0} bytes")]
    TooShort(usize),
    #[error("data does not start with the event CPI tag")]
    NotAnEvent,
    #[error("unknown instruction discriminator {0:?}")]
    UnknownInstruction([u8; 8]),
    #[error("unknown event discriminator {0:?}")]
    UnknownEvent([u8; 8]),
    #[error("unknown account discriminator {0:?}")]
    UnknownAccount([u8; 8]),
    #[error("could not decode {what}: {source}")]
    Borsh {
        what: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid transaction: {0}")]
    Transaction(String),
    #[error("{signature}: {position}: {source}")]
    At {
        signature: String,
        position: String,
        #[source]
        source: Box<DecodeError>,
    },
}

/// One of the program's custom error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProgramError {
    pub code: u32,
    pub name: &'static str,
    pub message: &'static str,
}

macro_rules! program_errors {
    ($( $code:literal $name:ident $message:literal ),* $(,)?) => {
        /// Every custom error the program defines, in IDL order.
        pub const PROGRAM_ERRORS: &[ProgramError] = &[
            $( ProgramError { code: $code, name: stringify!($name), message: $message } ),*
        ];
    };
}

program_errors! {
    6000 InvalidAdmin "Invalid admin",
    6001 InvalidOwner "Invalid owner",
    6002 InvalidConfigId "Invalid config id",
    6003 InvalidBump "Invalid bump",
    6004 InvalidDebouncerPda "Invalid debouncer PDA address",
    6005 InvalidPda "Invalid PDA address",
    6006 InvalidMint "Invalid mint",
    6007 InvalidState "Invalid state",
    6008 ArithmeticOverflow "Arithmetic overflow",
    6009 InvalidMessageLength "Invalid message length",
    6010 AccountInitialized "The account is already initialized",
    6011 EpochAlreadyClosed "Epoch already closed",
    6012 InvalidBalance "Invalid balance",
    6013 InvalidTokenAccountState "Invalid token account state",
    6014 InvalidTokenAccountOwner "Invalid token account owner",
    6015 SignerIsNotRelayer "Signer is not the relayer",
    6016 TransferFeeExtensionNotSupported "Transfer fee extension not supported",
    6017 FairDistributionError "Fair distribution error",
    6018 InvalidBps "Invalid bps",
    6019 InvalidInstructionData "Invalid instruction data",
    6020 InvalidInstructionDataLength "Invalid instruction data length",
    6021 InputAmountMismatch "Input amount mismatch",
    6022 BpsSumDoesNotEqualTenThousand "Bps sum does not equal ten thousand",
    6023 OutputAmountMustBeGreaterThanZero "Output amount must be greater than zero",
    6024 InvalidTokenAccountMint "Invalid token account mint",
    6025 InvalidTokenAccountProgram "Invalid token account program",
    6026 DebouncerNotReadyToDistribute "Debouncer not ready to distribute",
    6027 InvalidEpoch "Invalid epoch",
    6028 EpochNotClosed "Epoch not closed - cannot distribute",
    6029 InvalidTokenProgram "Invalid token program",
    6030 InsufficientRemainingAccounts "Insufficient remaining accounts",
    6031 MintNotSafeForDonation "Mint not safe for donation",
    6032 InvalidMintWhitelist "Mint whitelist account invalid",
    6033 SwapReceiptMismatch "Swap receipt does not match this distribution",
}

impl ProgramError {
    pub fn from_code(code: u32) -> Option<Self> {
        PROGRAM_ERRORS.iter().copied().find(|e| e.code == code)
    }
}
