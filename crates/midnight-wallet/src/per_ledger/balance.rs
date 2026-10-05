use midnight_types::balance::{
    DustBalance, ShieldedBalance, ShieldedCoinBalance, SpendableShieldedCoin, UnshieldedUtxoInfo,
    WalletBalance,
};
use midnight_types::{HashOutput, ShieldedTokenType, UnshieldedTokenType};

use super::helpers;
use super::state::Wallet;
use super::types::convert::IntoSdk;
use helpers::Timestamp;

impl Wallet {
    pub fn balance(&self) -> WalletBalance {
        WalletBalance {
            dust: self.dust_balance(),
            unshielded: self.unshielded_balance(),
            shielded: self.shielded_balance(),
        }
    }

    pub fn dust_balance(&self) -> DustBalance {
        let confirmed = self.dust_wallet().dust_local_state.as_deref();
        let unreserved = confirmed.map(|state| {
            self.reserved_dust_nullifiers()
                .fold(state.clone(), |state, nullifier| {
                    state
                        .remove_utxo(nullifier)
                        .expect("remove_utxo returns Ok in every ledger generation")
                })
        });
        let now = self
            .block_context()
            .map(|bc| bc.tblock)
            .unwrap_or_else(|| Timestamp::from_secs(0));
        let balance_speck = confirmed.map(|s| s.wallet_balance(now)).unwrap_or(0);
        let spendable_speck = unreserved
            .as_ref()
            .map(|s| s.wallet_balance(now))
            .unwrap_or(0);
        let spendable_utxos = unreserved.as_ref().map(|s| s.utxos().count()).unwrap_or(0);
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
            spendable_utxos,
            balance_speck,
            spendable_speck,
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

    /// See [`crate::Wallet::spendable_shielded_coins`].
    pub fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        // Exclude coins a recent still-pending build already spent, so callers
        // (and the pinned-coin validation in the contract-call builder) don't
        // re-select a coin that is no longer available.
        let reserved: std::collections::HashSet<helpers::Nullifier> =
            self.reserved_shielded_nullifiers().copied().collect();
        self.zswap_state()
            .coins
            .iter()
            .filter(|(nullifier, _)| !reserved.contains(nullifier))
            .map(|(nullifier, coin)| SpendableShieldedCoin {
                token_type: ShieldedTokenType(coin.type_.into_inner()),
                value: coin.value,
                nonce: coin.nonce.0.0,
                nullifier: nullifier.into_sdk(),
            })
            .collect()
    }
}
