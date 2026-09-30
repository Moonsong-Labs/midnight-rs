//! The transfer machinery on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.

pub(crate) use midnight_helpers::ledger_8 as helpers;

#[path = "per_ledger/builder.rs"]
mod builder;
#[path = "per_ledger/convert.rs"]
pub mod convert;
#[path = "per_ledger/inputs.rs"]
mod inputs;

pub use builder::{
    BuildInputs, BuiltTransaction, DustSpendBatch, PreparedTransfer, TransferBuilder,
    balance_external, build_no_validate, prepare_no_validate, prove_tx_no_validate,
    shielded_destination,
};
pub use inputs::{PreparedInput, prepare_shielded_inputs};

/// The generation this module builds for.
pub const LEDGER: crate::LedgerVersion = crate::LedgerVersion::V8;
