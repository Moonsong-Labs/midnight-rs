//! A wallet's builds on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.

use midnight_helpers::ledger_8 as helpers;
use midnight_types::ledger_8 as types;

#[path = "per_ledger/builds.rs"]
mod builds;

pub use builds::{ReservedBuild, WalletBuilds};
