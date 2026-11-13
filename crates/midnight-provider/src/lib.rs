#![warn(missing_debug_implementations)]

mod error;
mod local_prover;
mod node_error;
mod pipeline;
mod prover;
mod provider;
mod proving;
mod remote_prover;
mod submit;
mod types;

pub use error::ProviderError;
pub use pipeline::merge_transactions;
pub use prover::Prover;
pub use provider::{MidnightProvider, NodeHeader};
pub use submit::{NotApplied, PendingTx, PreparedTx, SubmitError, TransactionReceipt, Verdict};
pub use types::TransactionHash;

// Re-export the types that `MidnightProvider`'s own signatures name, so a
// caller of the provider needs no separate dep for them. `CoinSelection` comes
// with them: a `TransactionRequest` method takes it, and no other facade path
// reaches it. The implementation types (`Wallet`, `WalletBuilder`) live in
// midnight-wallet; this crate never names them.
pub use midnight_types::{
    BlockHash, CoinSelection, ExtrinsicHash, MidnightWallet, TransactionRequest, TxEnvelope,
    WalletError,
};

// Re-export the private-state types so callers configure
// `MidnightProvider::with_private_state` without a separate dep.
pub use midnight_private_state::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};
