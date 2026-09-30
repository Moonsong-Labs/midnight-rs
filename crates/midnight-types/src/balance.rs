//! What a wallet holds, as owned values: the three balance legs and the
//! spendable shielded coins.

use std::fmt;

use crate::{NIGHT, Nullifier, ShieldedTokenType, UnshieldedTokenType};

#[derive(Debug, Clone, Default)]
pub struct DustBalance {
    pub spendable_utxos: usize,
    /// Current dust balance in SPECK (1 DUST = 10^15 SPECK).
    /// Computed at the time of the balance query using UTXO age and generation parameters.
    pub balance_speck: u128,
    /// Whether any tNIGHT this wallet holds generates dust.
    ///
    /// Derived from the coins on hand, which is all the indexer reports, so it
    /// is not the same as "the address is registered". A wallet that registers
    /// and then spends every tNIGHT reads `false` here while its registration
    /// still stands, and tNIGHT arriving later still generates. Use
    /// [`Self::unregistered_night_utxos`] to decide whether there is anything
    /// left to register.
    pub night_generates_dust: bool,
    /// How many tNIGHT UTXOs still generate nothing, which is how many more
    /// `register_dust` calls this wallet needs. A registration covers the one
    /// UTXO it spends and every coin arriving afterwards, so this counts down
    /// rather than clearing at once.
    pub unregistered_night_utxos: usize,
}

#[derive(Debug, Clone)]
pub struct UnshieldedUtxoInfo {
    /// The UTXO's typed token id. Use [`token_type_hex`](Self::token_type_hex)
    /// for display / log output.
    pub token_type: UnshieldedTokenType,
    pub value: u128,
}

impl UnshieldedUtxoInfo {
    /// 64-char hex representation of the token id (no `0x` prefix), suitable
    /// for human-readable logs / debug output.
    pub fn token_type_hex(&self) -> String {
        hex::encode(self.token_type.0.0)
    }
}

impl fmt::Display for UnshieldedUtxoInfo {
    /// Render as `NIGHT: <value>` when the token is the native unshielded
    /// asset, otherwise `<8-char-hex-prefix>…: <value>`. The "t"-prefixed
    /// testnet name is a network convention the SDK can't infer from the
    /// token id alone — callers running against testnet can format manually
    /// using [`token_type_hex`](Self::token_type_hex) and [`value`](Self::value).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.token_type == NIGHT {
            write!(f, "NIGHT: {}", self.value)
        } else {
            let hex = self.token_type_hex();
            write!(f, "{}…: {}", &hex[..8], self.value)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShieldedCoinBalance {
    /// The coin's typed token id. Use [`token_type_hex`](Self::token_type_hex)
    /// for display / log output. Treat shielded token ids as opaque — the
    /// zero id is **not** NIGHT; see `docs/tokens.md`.
    pub token_type: ShieldedTokenType,
    pub value: u128,
}

impl ShieldedCoinBalance {
    /// 64-char hex representation of the token id (no `0x` prefix), suitable
    /// for human-readable logs / debug output.
    pub fn token_type_hex(&self) -> String {
        hex::encode(self.token_type.0.0)
    }
}

impl fmt::Display for ShieldedCoinBalance {
    /// Render as `<8-char-hex-prefix>…: <value>`. There is no shielded NIGHT;
    /// all shielded token ids are treated as opaque.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.token_type_hex();
        write!(f, "{}…: {}", &hex[..8], self.value)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ShieldedBalance {
    pub coins: Vec<ShieldedCoinBalance>,
    pub total_count: usize,
}

/// A single spendable shielded coin, addressed by its full coin info.
///
/// Unlike [`ShieldedCoinBalance`] (which aggregates by token type and carries no
/// nonce), this names one concrete coin: the `nonce`, `token_type`, and `value`
/// are exactly the `ShieldedCoinInfo { nonce, color, value }` a circuit argument
/// needs, and `nullifier` pins this exact coin when it is spent as a shielded
/// input. A circuit like `receiveShielded(coin)` re-commits the coin's exact
/// `nonce`/`color`/`value`, so the caller must both name the precise coin and
/// spend that same one — amount-based selection cannot express that, and this
/// accessor can. Enumerate with `WalletFacade::spendable_shielded_coins`.
#[derive(Debug, Clone)]
pub struct SpendableShieldedCoin {
    /// The coin's typed token id (its "color"). Treat shielded token ids as
    /// opaque; see [`ShieldedCoinBalance::token_type`].
    pub token_type: ShieldedTokenType,
    pub value: u128,
    /// The coin's 32-byte nonce, needed to build a `ShieldedCoinInfo` circuit
    /// argument that re-commits this exact coin.
    pub nonce: [u8; 32],
    /// Pins this exact coin when it is selected as a shielded input, so the SDK
    /// spends this coin and not another of the same token type / value.
    pub nullifier: Nullifier,
}

impl SpendableShieldedCoin {
    /// 64-char hex of the token id (no `0x` prefix), for logs / debug output.
    pub fn token_type_hex(&self) -> String {
        hex::encode(self.token_type.0.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct WalletBalance {
    pub dust: DustBalance,
    pub unshielded: Vec<UnshieldedUtxoInfo>,
    pub shielded: ShieldedBalance,
}
