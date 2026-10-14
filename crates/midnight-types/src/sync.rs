//! What a wallet's sync maintains, as its consumers read it: the unshielded
//! UTXOs it tracks and the cursors that say how far it has reached.

use midnight_helpers::HashOutput;

use crate::{NIGHT, SpentUtxoKey, UnshieldedTokenType};

/// How far a wallet's sync has reached. See `WalletFacade::sync_cursors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncCursors {
    /// Height of the latest block seen in an unshielded transaction event.
    ///
    /// This is NOT a general chain-sync cursor. It only advances when the
    /// wallet's unshielded address appears in a transaction.
    pub last_block_height: i64,
    /// Indexer id of the latest transaction the wallet applied.
    pub last_tx_id: Option<i64>,
    /// Highest zswap event id the wallet applied.
    pub zswap_event_id: i64,
    /// Highest dust event id the wallet applied.
    pub dust_event_id: i64,
}

/// A tracked unshielded UTXO from the indexer.
#[derive(Debug, Clone)]
pub struct TrackedUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: u128,
    pub intent_hash: Option<String>,
    pub output_index: Option<i64>,
    /// Creation time in seconds since the epoch. Dust generation accrues from
    /// this instant, so a dust registration needs it to declare the fee
    /// allowance the ledger will accept.
    pub ctime: Option<i64>,
    /// Whether this UTXO already generates dust. The ledger grants no
    /// generationless availability for such a UTXO, so a registration must
    /// leave it out.
    pub registered_for_dust_generation: Option<bool>,
}

/// The NIGHT token id in the 64-char hex form the indexer reports.
pub(crate) static NIGHT_TOKEN_HEX: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| NIGHT.to_string());

impl TrackedUtxo {
    /// Whether this UTXO holds the native NIGHT token.
    pub fn is_night(&self) -> bool {
        self.token_type == *NIGHT_TOKEN_HEX
    }

    /// The token this UTXO holds, parsed from its `token_type` hex. `None` on
    /// a decode error or wrong length.
    pub fn unshielded_token_type(&self) -> Option<UnshieldedTokenType> {
        hash_from_hex(&self.token_type).map(UnshieldedTokenType)
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
    ///
    /// `None` when the intent hash or the output index is missing, or when
    /// the index is outside `u32`. Such a UTXO cannot match a reservation.
    pub fn spent_key(&self) -> Option<SpentUtxoKey> {
        Some(SpentUtxoKey {
            intent_hash: self.intent_hash.clone()?,
            output_index: u32::try_from(self.output_index?).ok()?,
        })
    }
}

/// Parse the 64-char hex form of an intent hash, as [`TrackedUtxo`] carries
/// it. `None` on a decode error or wrong length.
pub fn parse_intent_hash_hex(hex: &str) -> Option<HashOutput> {
    hash_from_hex(hex)
}

fn hash_from_hex(hex: &str) -> Option<HashOutput> {
    Some(HashOutput(hex::decode(hex).ok()?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utxo(intent_hash: Option<&str>, output_index: Option<i64>) -> TrackedUtxo {
        TrackedUtxo {
            owner: "owner".into(),
            token_type: "00".repeat(32),
            value: 1,
            intent_hash: intent_hash.map(str::to_string),
            output_index,
            ctime: None,
            registered_for_dust_generation: None,
        }
    }

    /// Only a UTXO that carries both identity fields, with an index inside
    /// `u32`, has a key. A truncated index would name another output.
    #[test]
    fn spent_key_needs_both_identity_fields_in_range() {
        assert_eq!(
            utxo(Some("abcd"), Some(2)).spent_key(),
            Some(SpentUtxoKey {
                intent_hash: "abcd".into(),
                output_index: 2,
            })
        );
        assert_eq!(utxo(None, Some(1)).spent_key(), None);
        assert_eq!(utxo(Some("ffff"), None).spent_key(), None);
        assert_eq!(utxo(Some("eeee"), Some(-1)).spent_key(), None);
    }
}
