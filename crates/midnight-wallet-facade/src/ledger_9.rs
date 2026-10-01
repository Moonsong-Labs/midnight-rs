//! A wallet's builds on a chain that runs ledger 9.
//!
//! The same source as [`crate::ledger_8`], compiled against ledger 9's types.

use midnight_helpers::ledger_9 as helpers;
use midnight_types::ledger_9 as types;

#[path = "per_ledger/builds.rs"]
mod builds;

pub use builds::{ReservedBuild, WalletBuilds};
