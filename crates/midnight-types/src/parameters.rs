//! The chain parameters a wallet reads, in the shape every ledger generation
//! shares.

use midnight_helpers::Duration;

/// The chain's ledger parameters that a wallet and its callers read.
///
/// A generation's full `LedgerParameters` hold more (the cost model, limits,
/// fee prices), which the builds read from their own generation's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainParameters {
    /// How Dust generates and decays.
    pub dust: DustParameters,
    /// How far in the future a transaction's time-to-live may reach.
    pub global_ttl: Duration,
}

/// How Dust generates from NIGHT and decays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DustParameters {
    /// The Dust capacity each star of NIGHT generates, in SPECK.
    pub night_dust_ratio: u64,
    /// How fast a UTXO's Dust generates, and decays once the UTXO is spent,
    /// in SPECK per star per second.
    pub generation_decay_rate: u32,
    /// How long before the block's time a transaction's Dust actions may be
    /// dated.
    pub dust_grace_period: Duration,
}
