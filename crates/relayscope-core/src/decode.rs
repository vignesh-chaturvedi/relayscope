//! Decodes every Donation Relay instruction and event in a transaction, wherever it appears.
//!
//! Most donations do not call the relay directly. Another program calls it through CPI, and
//! the relay then emits its events through a further self-CPI. The decoder walks the call
//! stack, so each instruction records which program invoked it, and each event records the
//! relay instruction that emitted it.

use serde::Serialize;

use crate::error::{DecodeError, ProgramError};
use crate::events::Event;
use crate::instructions::Instruction;
use crate::program::{IDL_IX_TAG, PROGRAM_ID};
use crate::pubkey::Pubkey;
use crate::tx::{Position, TokenBalanceChange, TxView};

/// An account passed to an instruction, with its IDL name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NamedAccount {
    pub name: &'static str,
    pub address: Pubkey,
}

/// A relay instruction found in a transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecodedInstruction {
    pub position: Position,
    pub stack_height: u32,
    /// The program that invoked the relay, or `None` if the relay was called directly.
    pub invoked_by: Option<Pubkey>,
    #[serde(flatten)]
    pub instruction: Instruction,
    pub accounts: Vec<NamedAccount>,
    /// Accounts beyond the ones the IDL names, such as Jupiter's route accounts.
    pub remaining_accounts: Vec<Pubkey>,
}

impl DecodedInstruction {
    /// The address passed for the account the IDL calls `name`.
    pub fn account(&self, name: &str) -> Option<Pubkey> {
        self.accounts
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.address)
    }
}

/// A relay event found in a transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecodedEvent {
    pub position: Position,
    /// Index into [`DecodedTx::instructions`] of the relay instruction that emitted it.
    pub emitted_by: Option<usize>,
    #[serde(flatten)]
    pub event: Event,
}

/// Everything the relay did in one transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecodedTx {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub fee_payer: Pubkey,
    pub succeeded: bool,
    /// The relay's own error, if the transaction failed inside the relay.
    pub program_error: Option<ProgramError>,
    pub instructions: Vec<DecodedInstruction>,
    pub events: Vec<DecodedEvent>,
    /// Anchor's built-in IDL management instructions, which update the IDL account and never
    /// touch donations. They are recorded so the transaction is accounted for, not decoded.
    pub idl_management: Vec<Position>,
    /// Balance changes of every token account the transaction touched.
    pub token_balances: Vec<TokenBalanceChange>,
}

impl DecodedTx {
    /// The decoded instruction that emitted an event.
    pub fn emitter(&self, event: &DecodedEvent) -> Option<&DecodedInstruction> {
        event.emitted_by.and_then(|i| self.instructions.get(i))
    }

    /// Whether the relay appears in this transaction at all.
    pub fn touches_relay(&self) -> bool {
        !self.instructions.is_empty() || !self.events.is_empty() || !self.idl_management.is_empty()
    }
}

/// One frame of the call stack while walking a transaction.
struct Frame {
    stack_height: u32,
    program_id: Pubkey,
    /// Set when this frame is a decoded relay instruction.
    relay_instruction: Option<usize>,
}

/// Decodes every relay instruction and event in a transaction.
///
/// Decoding is strict: an unknown discriminator, or data that does not match the layout
/// exactly, is an error. A changed program should stop the indexer, not be misread.
pub fn decode_transaction(tx: &TxView) -> Result<DecodedTx, DecodeError> {
    let mut instructions = Vec::new();
    let mut events = Vec::new();
    let mut idl_management = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    for ix in &tx.instructions {
        while stack
            .last()
            .is_some_and(|frame| frame.stack_height >= ix.stack_height)
        {
            stack.pop();
        }
        let caller = stack.last();
        let mut relay_instruction = None;

        if ix.program_id == PROGRAM_ID {
            let at = |source| DecodeError::At {
                signature: tx.signature.clone(),
                position: ix.position.to_string(),
                source: Box::new(source),
            };
            if ix.data.starts_with(&IDL_IX_TAG) {
                idl_management.push(ix.position);
            } else if Event::is_event_data(&ix.data) {
                let event = Event::decode(&ix.data).map_err(at)?;
                events.push(DecodedEvent {
                    position: ix.position,
                    emitted_by: caller.and_then(|frame| frame.relay_instruction),
                    event,
                });
            } else {
                let instruction = Instruction::decode(&ix.data).map_err(at)?;
                let names = instruction.account_names();
                let named = ix.accounts.len().min(names.len());
                relay_instruction = Some(instructions.len());
                instructions.push(DecodedInstruction {
                    position: ix.position,
                    stack_height: ix.stack_height,
                    invoked_by: caller.map(|frame| frame.program_id),
                    accounts: names
                        .iter()
                        .zip(&ix.accounts)
                        .map(|(name, address)| NamedAccount {
                            name,
                            address: *address,
                        })
                        .collect(),
                    remaining_accounts: ix.accounts[named..].to_vec(),
                    instruction,
                });
            }
        }

        stack.push(Frame {
            stack_height: ix.stack_height,
            program_id: ix.program_id,
            relay_instruction,
        });
    }

    Ok(DecodedTx {
        signature: tx.signature.clone(),
        slot: tx.slot,
        block_time: tx.block_time,
        fee_payer: tx.fee_payer,
        succeeded: tx.error.is_none(),
        program_error: relay_error(&tx.log_messages),
        instructions,
        events,
        idl_management,
        token_balances: tx.token_balances.clone(),
    })
}

/// Finds the relay's custom error in the logs, such as
/// `Program RLAY... failed: custom program error: 0x1772`.
fn relay_error(logs: &[String]) -> Option<ProgramError> {
    let prefix = format!("Program {PROGRAM_ID} failed: custom program error: 0x");
    logs.iter().find_map(|line| {
        let hex = line.strip_prefix(&prefix)?;
        let code = u32::from_str_radix(hex.trim(), 16).ok()?;
        ProgramError::from_code(code)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_relay_error_from_logs() {
        let logs = vec![
            "Program RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu invoke [1]".to_string(),
            "Program RLAYHr9TRFcKB2ubYQhspcnXiaGpaVzNQvHytt47RZu failed: custom program error: 0x177b"
                .to_string(),
        ];
        let error = relay_error(&logs).unwrap();
        assert_eq!(error.code, 6011);
        assert_eq!(error.name, "EpochAlreadyClosed");
    }

    #[test]
    fn ignores_other_programs_errors() {
        let logs = vec![
            "Program 11111111111111111111111111111111 failed: custom program error: 0x1"
                .to_string(),
        ];
        assert_eq!(relay_error(&logs), None);
    }
}
