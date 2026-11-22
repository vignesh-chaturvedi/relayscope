//! Types shared by events, instructions and accounts.

use crate::pubkey::Pubkey;
use crate::shape::{idl_enum, idl_struct};

idl_enum! {
    /// Who a payout line is for.
    pub enum ShareType {
        Beneficiary,
        Fee,
        Tip,
    }
}

idl_enum! {
    /// The first field of every event. It repeats the event's kind.
    pub enum DaasEventDiscriminator {
        DonationV1,
        DonationEpochClosedV1,
        DonationEpochProcessedV1,
        DonationEpochJustSwappedV1,
        DonationEpochProcessedExactV1,
        BridgeDonationAdmittedV1,
        BridgePaymentRecoveredV1,
    }
}

idl_struct! {
    /// A payout line by basis points (older payouts).
    pub struct Share {
        pub share_type: ShareType,
        pub address: Pubkey,
        pub bps: u16,
    }
}

idl_struct! {
    /// A paid line of an older bps payout: the address, the amount and the bps it was computed from.
    pub struct Receipt {
        pub share_type: ShareType,
        pub address: Pubkey,
        pub amount: u64,
        pub bps: u16,
    }
}

idl_struct! {
    /// A payout line with an exact amount (current payouts).
    pub struct ExactTransfer {
        pub share_type: ShareType,
        pub address: Pubkey,
        pub amount: u64,
    }
}

idl_enum! {
    pub enum DebouncerV1State {
        Uninitialized,
        Initialized,
    }
}

idl_enum! {
    pub enum DistributionV1State {
        Uninitialized,
        Initialized,
    }
}

idl_enum! {
    pub enum EpochTrackerV1State {
        Uninitialized,
        Initialized,
    }
}
