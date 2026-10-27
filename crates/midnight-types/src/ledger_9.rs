//! The transfer machinery on a chain that runs ledger 9.
//!
//! The same source as [`crate::ledger_8`], compiled against ledger 9's types.

pub(crate) use midnight_helpers::ledger_9 as helpers;

#[path = "per_ledger/builder.rs"]
mod builder;
#[path = "per_ledger/convert.rs"]
pub mod convert;
#[path = "per_ledger/inputs.rs"]
mod inputs;

pub(crate) use builder::shielded_destination;
pub use builder::{
    BuildInputs, BuiltTransaction, DustSpendBatch, PreparedTransfer, TransferBuilder,
    balance_external, balance_external_tokens, build_no_validate, fee_speck, prepare_no_validate,
    prove_tx_no_validate,
};
pub use inputs::{PreparedInput, prepare_shielded_inputs};

/// The generation this module builds for.
pub const LEDGER: crate::LedgerVersion = crate::LedgerVersion::V9;
