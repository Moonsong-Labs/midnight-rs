//! The unshielded UTXOs a wallet's sync tracks.

use midnight_helpers::HashOutput;

use crate::{NIGHT, SpentUtxoKey, UnshieldedTokenType};

/// A tracked unshielded UTXO from the indexer.
#[derive(Debug, Clone)]
pub struct TrackedUtxo {
    pub owner: String,
    /// The token this UTXO holds.
    pub token_type: UnshieldedTokenType,
    pub value: u128,
    /// The intent hash of the transaction that created this UTXO.
    pub intent_hash: HashOutput,
    /// The number of this UTXO among its transaction's outputs, as the ledger
    /// counts it.
    pub output_index: u32,
    /// Creation time in seconds since the epoch. Dust generation accrues from
    /// this instant, so a dust registration needs it to declare the fee
    /// allowance the ledger will accept.
    pub ctime: Option<i64>,
    /// Whether this UTXO already generates dust. The ledger grants no
    /// generationless availability for such a UTXO, so a registration must
    /// leave it out.
    pub registered_for_dust_generation: Option<bool>,
}

impl TrackedUtxo {
    /// Whether this UTXO holds the native NIGHT token.
    pub fn is_night(&self) -> bool {
        self.token_type == NIGHT
    }

    /// Whether this UTXO already generates dust.
    ///
    /// An absent flag reads as not registered, which is what makes a
    /// registration build include the UTXO. Read it through here rather than
    /// comparing the field, so a caller asking "is this registered" and the
    /// builder asking "does this still need registering" cannot disagree.
    pub fn is_registered_for_dust(&self) -> bool {
        self.registered_for_dust_generation == Some(true)
    }

    /// The key that a reservation of this UTXO and a confirmed spend of it
    /// share, so that the confirmation clears the reservation.
    pub fn spent_key(&self) -> SpentUtxoKey {
        SpentUtxoKey {
            intent_hash: self.intent_hash,
            output_index: self.output_index,
        }
    }
}
