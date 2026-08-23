//! What a wallet's sync maintains, as its consumers read it: the unshielded
//! UTXOs it tracks and the cursors that say how far it has reached.

use midnight_helpers::NIGHT;

use crate::WalletError;

/// How far a wallet's sync has reached. See `Wallet::sync_cursors`.
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
static NIGHT_TOKEN_HEX: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| hex::encode(NIGHT.0.0));

impl TrackedUtxo {
    /// Whether this UTXO holds the native NIGHT token.
    pub fn is_night(&self) -> bool {
        self.token_type == *NIGHT_TOKEN_HEX
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
}

impl TryFrom<midnight_indexer_client::UnshieldedUtxo> for TrackedUtxo {
    type Error = WalletError;

    fn try_from(utxo: midnight_indexer_client::UnshieldedUtxo) -> Result<Self, Self::Error> {
        let value: u128 = utxo.value.parse().map_err(|e| {
            WalletError::Sync(format!("failed to parse UTXO value '{}': {e}", utxo.value))
        })?;
        Ok(Self {
            owner: utxo.owner,
            token_type: utxo.token_type,
            value,
            intent_hash: utxo.intent_hash,
            output_index: utxo.output_index,
            ctime: utxo.ctime,
            registered_for_dust_generation: utxo.registered_for_dust_generation,
        })
    }
}

/// Parse the 64-char hex form of an intent hash, as [`TrackedUtxo`] carries
/// it. `None` on a decode error or wrong length.
pub fn parse_intent_hash_hex(hex: &str) -> Option<midnight_helpers::IntentHash> {
    let arr: [u8; 32] = hex::decode(hex).ok()?.try_into().ok()?;
    Some(midnight_helpers::IntentHash(midnight_helpers::HashOutput(
        arr,
    )))
}
