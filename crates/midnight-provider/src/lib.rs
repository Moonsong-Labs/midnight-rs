#![warn(missing_debug_implementations)]

mod error;
mod local_prover;
pub mod node_error;
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

// Re-export the coin, token, key and hash vocabulary, the types that
// `MidnightProvider`'s own signatures name, and the NIGHT and DUST units, so a
// caller of the provider needs no separate dep for them. The implementation
// types (`Wallet`, `WalletBuilder`) live in midnight-wallet; this crate never
// names them.
pub use midnight_types::{
    BlockHash, CoinInfo, CoinPublicKey, CoinSelection, ContractAddress, DustPublicKey,
    EncryptionPublicKey, ExtrinsicHash, HashOutput, MidnightWallet, NIGHT, Nonce, Nullifier,
    SPECKS_PER_DUST, STARS_PER_NIGHT, ShieldedRecipient, ShieldedTokenType, TransactionRequest,
    TxEnvelope, UnshieldedRecipient, UnshieldedTokenType, WalletError,
};

// Re-export the private-state types so callers configure
// `MidnightProvider::with_private_state` without a separate dep.
pub use midnight_private_state::{
    ConflictStrategy, EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions,
    ImportResult, PrivateStateError, PrivateStateProvider, Snapshot, SnapshotStatus,
};
