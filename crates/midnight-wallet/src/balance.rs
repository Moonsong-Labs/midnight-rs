pub use midnight_types::balance::*;
use midnight_types::{HashOutput, ShieldedTokenType, Timestamp, UnshieldedTokenType};

use crate::state::Wallet;

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
        // Both readings look at tNIGHT alone, because that is what generates
        // dust and what `register_dust` selects from. One pass, so they cannot
        // come to disagree.
        let (night_generates_dust, unregistered_night_utxos) = self
            .unshielded_utxos()
            .iter()
            .filter(|u| u.is_night())
            .fold((false, 0), |(any, count), u| {
                if u.is_registered_for_dust() {
                    (true, count)
                } else {
                    (any, count + 1)
                }
            });
        DustBalance {
            spendable_utxos: count,
            balance_speck,
            night_generates_dust,
            unregistered_night_utxos,
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
        // Exclude coins a recent still-pending build already spent, so callers
        // (and the pinned-coin validation in the contract-call builder) don't
        // re-select a coin that is no longer available.
        let reserved: std::collections::HashSet<midnight_types::Nullifier> =
            self.reserved_shielded_nullifiers().copied().collect();
        self.zswap_state()
            .coins
            .iter()
            .map(|(nullifier, coin)| SpendableShieldedCoin {
                token_type: ShieldedTokenType(coin.type_.into_inner()),
                value: coin.value,
                nonce: coin.nonce.0.0,
                nullifier,
            })
            .filter(|c| !reserved.contains(&c.nullifier))
            .collect()
    }
}
