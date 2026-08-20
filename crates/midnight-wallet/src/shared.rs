//! A [`Wallet`] held by more than one consumer.
//!
//! The wallet is a state machine with no I/O of its own, so whoever drives it
//! decides when to read, when to mutate, and when to replay events into it.
//! Once two drivers hold the same wallet, those decisions have to be
//! serialized somewhere, and the wallet is the only party that knows what its
//! own state needs. That is what this module owns.

use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::state::Wallet;

/// A [`Wallet`] and the locking that lets several consumers drive it.
///
/// Clone it once per consumer. Every clone reads and writes the same wallet, so
/// they see each other's pending reservations: a build that reserves an input
/// in one consumer stops another from re-selecting it. Two separately synced
/// wallets on one seed do not have that, and the node rejects the second
/// build's transaction for spending what the first already spent.
///
/// ```rust,ignore
/// let shared = SharedWallet::from(wallet);
/// let a = MidnightProvider::new(node, indexer)?.with_wallet(shared.clone());
/// let b = MidnightProvider::new(node, indexer)?.with_wallet(shared);
/// ```
///
/// This is one process. Separate processes hold separate wallets whatever they
/// do with this type.
#[derive(Clone)]
pub struct SharedWallet {
    wallet: Arc<RwLock<Wallet>>,
    /// Serializes event replay. A replay snapshots the wallet's cursors, does
    /// its I/O without the wallet lock so reads keep flowing, then commits.
    /// Two replays interleaving that way would resume from the same cursors
    /// and race their commits, so a replay holds this across the whole
    /// sequence. Per wallet, not per consumer: consumers must serialize
    /// against each other, not only against themselves.
    replay_lock: Arc<Mutex<()>>,
}

impl From<Wallet> for SharedWallet {
    fn from(wallet: Wallet) -> Self {
        Self {
            wallet: Arc::new(RwLock::new(wallet)),
            replay_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl SharedWallet {
    /// Read the wallet. Release the guard promptly: a writer waits behind it.
    pub async fn read(&self) -> RwLockReadGuard<'_, Wallet> {
        self.wallet.read().await
    }

    /// Mutate the wallet. Release the guard promptly: readers wait behind it.
    pub async fn write(&self) -> RwLockWriteGuard<'_, Wallet> {
        self.wallet.write().await
    }

    /// Claim the right to replay events into this wallet, blocking until any
    /// replay in flight has committed.
    ///
    /// Hold the guard across the whole sequence: a replay snapshots the
    /// wallet's cursors, does its I/O without the wallet lock so reads keep
    /// flowing, then commits. Two replays interleaving that way would resume
    /// from the same cursors and race their commits.
    pub async fn replay_guard(&self) -> MutexGuard<'_, ()> {
        self.replay_lock.lock().await
    }

    /// Whether these two handles drive the same wallet.
    pub fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.wallet, &other.wallet)
    }
}
