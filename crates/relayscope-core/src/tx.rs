//! A transaction in a source-independent form.
//!
//! Backfill reads transactions from JSON-RPC and live ingestion reads them from Yellowstone
//! gRPC. Both are converted into a [`TxView`], and the decoder only ever sees that.

use serde::{Deserialize, Serialize};

use crate::error::DecodeError;
use crate::pubkey::Pubkey;

/// Where an instruction sits in a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Position {
    /// Index of the top-level instruction.
    pub top: u16,
    /// Index within that instruction's inner instructions, or `None` for the top-level one.
    pub inner: Option<u16>,
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner {
            Some(inner) => write!(f, "instruction {}.{}", self.top, inner),
            None => write!(f, "instruction {}", self.top),
        }
    }
}

/// One instruction, top-level or inner, in execution order.
#[derive(Clone, Debug)]
pub struct InstructionView {
    pub position: Position,
    pub program_id: Pubkey,
    pub accounts: Vec<Pubkey>,
    pub data: Vec<u8>,
    /// Depth in the call stack: 1 for top-level, 2 for its first CPI level, and so on.
    pub stack_height: u32,
}

/// A token account's balance before and after the transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TokenBalanceChange {
    pub account: Pubkey,
    pub mint: Pubkey,
    pub owner: Option<Pubkey>,
    pub decimals: u8,
    pub pre: u64,
    pub post: u64,
}

/// A transaction with everything the decoder needs, independent of where it came from.
#[derive(Clone, Debug)]
pub struct TxView {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    /// The transaction error, as the node reported it. `None` means it succeeded.
    pub error: Option<serde_json::Value>,
    pub fee_payer: Pubkey,
    /// Every instruction in execution order: each top-level instruction, then its inner ones.
    pub instructions: Vec<InstructionView>,
    pub token_balances: Vec<TokenBalanceChange>,
    pub log_messages: Vec<String>,
}

impl TxView {
    /// Builds a view from a JSON-RPC `getTransaction` result fetched with `encoding: "json"`
    /// and `maxSupportedTransactionVersion: 0`.
    pub fn from_rpc_json(value: &serde_json::Value) -> Result<Self, DecodeError> {
        let tx: rpc::Transaction = serde_json::from_value(value.clone())
            .map_err(|e| DecodeError::Transaction(e.to_string()))?;
        tx.into_view()
    }
}

/// The subset of the JSON-RPC transaction format that Relayscope reads.
mod rpc {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Transaction {
        pub slot: u64,
        pub block_time: Option<i64>,
        pub transaction: Inner,
        pub meta: Option<Meta>,
    }

    #[derive(Deserialize)]
    pub struct Inner {
        pub signatures: Vec<String>,
        pub message: Message,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Message {
        pub account_keys: Vec<String>,
        pub instructions: Vec<Instruction>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Instruction {
        pub program_id_index: usize,
        pub accounts: Vec<usize>,
        pub data: String,
        pub stack_height: Option<u32>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Meta {
        pub err: Option<serde_json::Value>,
        #[serde(default)]
        pub inner_instructions: Option<Vec<InnerGroup>>,
        #[serde(default)]
        pub loaded_addresses: Option<LoadedAddresses>,
        #[serde(default)]
        pub pre_token_balances: Option<Vec<TokenBalance>>,
        #[serde(default)]
        pub post_token_balances: Option<Vec<TokenBalance>>,
        #[serde(default)]
        pub log_messages: Option<Vec<String>>,
    }

    #[derive(Deserialize)]
    pub struct InnerGroup {
        pub index: usize,
        pub instructions: Vec<Instruction>,
    }

    #[derive(Deserialize, Default)]
    pub struct LoadedAddresses {
        #[serde(default)]
        pub writable: Vec<String>,
        #[serde(default)]
        pub readonly: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TokenBalance {
        pub account_index: usize,
        pub mint: String,
        pub owner: Option<String>,
        pub ui_token_amount: UiTokenAmount,
    }

    #[derive(Deserialize)]
    pub struct UiTokenAmount {
        pub amount: String,
        pub decimals: u8,
    }

    fn parse_key(s: &str) -> Result<Pubkey, DecodeError> {
        s.parse()
            .map_err(|e| DecodeError::Transaction(format!("bad address {s}: {e}")))
    }

    fn parse_amount(s: &str) -> Result<u64, DecodeError> {
        s.parse()
            .map_err(|_| DecodeError::Transaction(format!("bad token amount {s}")))
    }

    impl Transaction {
        pub fn into_view(self) -> Result<TxView, DecodeError> {
            let signature = self
                .transaction
                .signatures
                .first()
                .cloned()
                .ok_or_else(|| DecodeError::Transaction("no signature".into()))?;
            let meta = self
                .meta
                .ok_or_else(|| DecodeError::Transaction("no status meta".into()))?;

            // Static keys first, then addresses loaded from lookup tables: writable, then readonly.
            let loaded = meta.loaded_addresses.unwrap_or_default();
            let keys = self
                .transaction
                .message
                .account_keys
                .iter()
                .chain(&loaded.writable)
                .chain(&loaded.readonly)
                .map(|k| parse_key(k))
                .collect::<Result<Vec<_>, _>>()?;
            let key = |index: usize| {
                keys.get(index).copied().ok_or_else(|| {
                    DecodeError::Transaction(format!("account index {index} out of range"))
                })
            };

            let view = |ix: &Instruction, position: Position, default_height: u32| {
                Ok::<_, DecodeError>(InstructionView {
                    position,
                    program_id: key(ix.program_id_index)?,
                    accounts: ix
                        .accounts
                        .iter()
                        .map(|&i| key(i))
                        .collect::<Result<_, _>>()?,
                    data: bs58::decode(&ix.data).into_vec().map_err(|_| {
                        DecodeError::Transaction(format!("{position}: data is not base58"))
                    })?,
                    stack_height: ix.stack_height.unwrap_or(default_height),
                })
            };

            let groups = meta.inner_instructions.unwrap_or_default();
            let mut instructions = Vec::new();
            for (top, ix) in self.transaction.message.instructions.iter().enumerate() {
                let top = u16::try_from(top).expect("fewer than 65536 instructions");
                instructions.push(view(ix, Position { top, inner: None }, 1)?);
                for group in groups.iter().filter(|g| g.index == usize::from(top)) {
                    for (inner, ix) in group.instructions.iter().enumerate() {
                        let inner = u16::try_from(inner).expect("fewer than 65536 instructions");
                        instructions.push(view(
                            ix,
                            Position {
                                top,
                                inner: Some(inner),
                            },
                            2,
                        )?);
                    }
                }
            }

            let pre = meta.pre_token_balances.unwrap_or_default();
            let post = meta.post_token_balances.unwrap_or_default();
            let mut indexes: Vec<usize> =
                pre.iter().chain(&post).map(|b| b.account_index).collect();
            indexes.sort_unstable();
            indexes.dedup();
            let mut token_balances = Vec::with_capacity(indexes.len());
            for index in indexes {
                let before = pre.iter().find(|b| b.account_index == index);
                let after = post.iter().find(|b| b.account_index == index);
                let any = after.or(before).expect("index came from one of the lists");
                token_balances.push(TokenBalanceChange {
                    account: key(index)?,
                    mint: parse_key(&any.mint)?,
                    owner: any.owner.as_deref().map(parse_key).transpose()?,
                    decimals: any.ui_token_amount.decimals,
                    pre: before
                        .map(|b| parse_amount(&b.ui_token_amount.amount))
                        .transpose()?
                        .unwrap_or(0),
                    post: after
                        .map(|b| parse_amount(&b.ui_token_amount.amount))
                        .transpose()?
                        .unwrap_or(0),
                });
            }

            Ok(TxView {
                signature,
                slot: self.slot,
                block_time: self.block_time,
                error: meta.err,
                fee_payer: key(0)?,
                instructions,
                token_balances,
                log_messages: meta.log_messages.unwrap_or_default(),
            })
        }
    }
}
