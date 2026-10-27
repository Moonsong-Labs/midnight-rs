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

pub use builder::{
    BuildInputs, DustSpendBatch, PreparedTransfer, TransferBuilder, balance_external, fee_speck,
    prepare_no_validate,
};
pub(crate) use builder::{parse_wallet_address, shielded_destination};
pub use inputs::{PreparedInput, prepare_shielded_inputs};

/// The generation this module builds for.
pub const LEDGER: crate::LedgerVersion = crate::LedgerVersion::V9;
