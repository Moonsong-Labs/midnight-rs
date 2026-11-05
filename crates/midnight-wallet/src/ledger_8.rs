//! A wallet's state on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.
//! [`crate::wallet::WalletState`] holds one generation's [`Wallet`] and moves
//! it to the next at a hard fork.

use midnight_helpers::ledger_8 as helpers;
use midnight_types::WalletError;
use midnight_types::ledger_8 as types;
use midnight_wallet_facade::ledger_8 as facade;
use types::LEDGER;

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

/// The state of `wallet`, when it is in this generation.
fn wallet_state(wallet: &crate::wallet::WalletState) -> Result<&Wallet, WalletError> {
    let expected = wallet.ledger_version();
    match wallet {
        crate::wallet::WalletState::Ledger8(state) => Ok(state),
        _ => Err(WalletError::LedgerMismatch {
            expected,
            found: LEDGER,
        }),
    }
}

/// [`wallet_state`], to change it.
fn wallet_state_mut(wallet: &mut crate::wallet::WalletState) -> Result<&mut Wallet, WalletError> {
    let expected = wallet.ledger_version();
    match wallet {
        crate::wallet::WalletState::Ledger8(state) => Ok(state),
        _ => Err(WalletError::LedgerMismatch {
            expected,
            found: LEDGER,
        }),
    }
}
