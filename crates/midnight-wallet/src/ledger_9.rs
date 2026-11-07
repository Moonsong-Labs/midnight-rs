//! A wallet's state on a chain that runs ledger 9. [`crate::Wallet`] holds
//! its [`Wallet`].

use midnight_helpers::ledger_9 as helpers;
use midnight_types::ledger_9 as types;
use midnight_wallet_facade::ledger_9 as facade;

#[path = "per_ledger/balance.rs"]
mod balance;
#[path = "per_ledger/builds.rs"]
mod builds;
#[path = "per_ledger/pending.rs"]
mod pending;
#[path = "per_ledger/snapshot.rs"]
pub(crate) mod snapshot;
#[path = "per_ledger/state.rs"]
pub mod state;
#[path = "per_ledger/transfer.rs"]
mod transfer;

pub use state::{ResyncCommit, ResyncPlan, ShieldedRescanCommit, ShieldedRescanPlan, Wallet};
