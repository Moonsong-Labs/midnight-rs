//! A wallet's state on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.
//! [`crate::Wallet`] holds one generation's [`Wallet`] and moves it to the
//! next at a hard fork.

use midnight_helpers::ledger_8 as helpers;
use midnight_types::ledger_8 as types;
use midnight_types::{LedgerVersion, WalletError};
use midnight_wallet_facade::ledger_8 as facade;

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

/// The generation this module keeps a wallet in.
pub(crate) const LEDGER: LedgerVersion = LedgerVersion::V8;

/// The state of `wallet`, when it is in this generation.
fn wallet_state(wallet: &crate::Wallet) -> Result<&Wallet, WalletError> {
    let expected = wallet.ledger_version();
    match wallet.state() {
        crate::wallet::State::Ledger8(state) => Ok(state),
        _ => Err(WalletError::LedgerMismatch {
            expected,
            found: LEDGER,
        }),
    }
}

/// [`wallet_state`], to change it.
fn wallet_state_mut(wallet: &mut crate::Wallet) -> Result<&mut Wallet, WalletError> {
    let expected = wallet.ledger_version();
    match wallet.state_mut() {
        crate::wallet::State::Ledger8(state) => Ok(state),
        _ => Err(WalletError::LedgerMismatch {
            expected,
            found: LEDGER,
        }),
    }
}
