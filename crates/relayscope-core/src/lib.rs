//! Decoding, data model and reconciliation for the Donation Relay program on Solana
//! (`RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu`).
//!
//! ```no_run
//! # fn run(rpc_result: serde_json::Value) -> Result<(), relayscope_core::DecodeError> {
//! use relayscope_core::{Event, TxView, decode_transaction};
//!
//! let tx = TxView::from_rpc_json(&rpc_result)?;
//! let decoded = decode_transaction(&tx)?;
//! for event in &decoded.events {
//!     if let Event::DonationMadeV1Event(donation) = &event.event {
//!         println!("{} base units to config {}", donation.gross_amount, donation.config_id);
//!     }
//! }
//! # Ok(()) }
//! ```

pub mod accounts;
pub mod decode;
pub mod error;
pub mod events;
pub mod instructions;
pub mod pda;
pub mod program;
pub mod pubkey;
pub mod shape;
pub mod tx;
pub mod types;

pub use accounts::{ACCOUNTS, Account};
pub use decode::{DecodedEvent, DecodedInstruction, DecodedTx, NamedAccount, decode_transaction};
pub use error::{DecodeError, PROGRAM_ERRORS, ProgramError};
pub use events::{EVENTS, Event};
pub use instructions::{INSTRUCTIONS, Instruction};
pub use program::{EVENT_IX_TAG, IDL_IX_TAG, PROGRAM_ID};
pub use pubkey::{Bytes, Bytes32, Pubkey};
pub use tx::{InstructionView, Position, TokenBalanceChange, TxView};
