use std::fmt;

use midnight_helpers::{HashOutput, NIGHT, ShieldedTokenType, Timestamp, UnshieldedTokenType};

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
}
