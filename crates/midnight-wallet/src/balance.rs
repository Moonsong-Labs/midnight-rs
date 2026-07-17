use std::fmt;

use midnight_helpers::{
    HashOutput, NIGHT, Nullifier, ShieldedTokenType, Timestamp, UnshieldedTokenType,
};

use crate::state::Wallet;

#[derive(Debug, Clone, Default)]
pub struct DustBalance {
    pub spendable_utxos: usize,
    /// Current dust balance in SPECK (1 DUST = 10^15 SPECK).
    /// Computed at the time of the balance query using UTXO age and generation parameters.
    pub balance_speck: u128,
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
    /// zero id is **not** NIGHT; see [`docs/tokens.md`].
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
/// accessor can. Enumerate with [`Wallet::spendable_shielded_coins`].
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

impl Wallet {
    pub fn balance(&self) -> WalletBalance {
        WalletBalance {
            dust: self.dust_balance(),
            unshielded: self.unshielded_balance(),
            shielded: self.shielded_balance(),
        }
    }

    pub fn dust_balance(&self) -> DustBalance {
        let local_state = self.dust_wallet().dust_local_state.as_ref();
        let count = local_state.map(|s| s.utxos().count()).unwrap_or(0);
        let now = self
            .block_context()
            .map(|bc| bc.tblock)
            .unwrap_or_else(|| Timestamp::from_secs(0));
        let balance_speck = local_state.map(|s| s.wallet_balance(now)).unwrap_or(0);
        DustBalance {
            spendable_utxos: count,
            balance_speck,
        }
    }

    pub fn unshielded_balance(&self) -> Vec<UnshieldedUtxoInfo> {
        self.unshielded_utxos()
            .iter()
            .filter_map(|utxo| {
                let bytes: [u8; 32] = hex::decode(&utxo.token_type).ok()?.try_into().ok()?;
                Some(UnshieldedUtxoInfo {
                    token_type: UnshieldedTokenType(HashOutput(bytes)),
                    value: utxo.value,
                })
            })
            .collect()
    }

    pub fn shielded_balance(&self) -> ShieldedBalance {
        let coins: Vec<ShieldedCoinBalance> = self
            .zswap_state()
            .coins
            .iter()
            .map(|(_nullifier, coin)| ShieldedCoinBalance {
                token_type: ShieldedTokenType(coin.type_.into_inner()),
                value: coin.value,
            })
            .collect();
        let total_count = coins.len();
        ShieldedBalance { coins, total_count }
    }

    /// Enumerate the wallet's spendable shielded coins, each with its full coin
    /// info (nonce, token type, value) plus the nullifier that pins it.
    ///
    /// Use this to address a specific coin for a circuit that spends it (e.g.
    /// `receiveShielded`): build the `ShieldedCoinInfo` argument from the coin's
    /// `nonce`/`token_type`/`value`, then hand the same coin back to the call
    /// builder so the SDK spends that exact coin as the shielded input. See
    /// [`SpendableShieldedCoin`].
    pub fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        self.zswap_state()
            .coins
            .iter()
            .map(|(nullifier, coin)| SpendableShieldedCoin {
                token_type: ShieldedTokenType(coin.type_.into_inner()),
                value: coin.value,
                nonce: coin.nonce.0.0,
                nullifier,
            })
            .collect()
    }
}
