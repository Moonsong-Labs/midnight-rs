use crate::state::WalletState;

#[derive(Debug, Clone, Default)]
pub struct DustBalance {
    pub spendable_utxos: usize,
}

#[derive(Debug, Clone)]
pub struct UnshieldedUtxoInfo {
    pub token_type: String,
    pub value: u128,
}

#[derive(Debug, Clone)]
pub struct ShieldedCoinBalance {
    pub token_type: String,
    pub value: u128,
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

impl WalletState {
    pub fn balance(&self) -> WalletBalance {
        WalletBalance {
            dust: self.dust_balance(),
            unshielded: self.unshielded_balance(),
            shielded: self.shielded_balance(),
        }
    }

    pub fn dust_balance(&self) -> DustBalance {
        // If we have a cached LedgerContext, use it for accurate dust count.
        // Otherwise, dust tracking from the indexer would require the
        // dustLedgerEvents subscription (not yet implemented).
        if let Some(ctx) = self.context() {
            let wallet = ctx
                .wallets
                .lock()
                .expect("lock wallets")
                .get(self.seed())
                .cloned();
            let count = wallet
                .and_then(|w| w.dust.dust_local_state.as_ref().map(|s| s.utxos().count()))
                .unwrap_or(0);
            DustBalance {
                spendable_utxos: count,
            }
        } else {
            DustBalance { spendable_utxos: 0 }
        }
    }

    pub fn unshielded_balance(&self) -> Vec<UnshieldedUtxoInfo> {
        self.unshielded_utxos()
            .iter()
            .map(|utxo| UnshieldedUtxoInfo {
                token_type: utxo.token_type.clone(),
                value: utxo.value,
            })
            .collect()
    }

    pub fn shielded_balance(&self) -> ShieldedBalance {
        // Shielded balance requires a viewing key session with the indexer
        // (connect mutation + shieldedTransactions subscription).
        // For now, fall back to the cached LedgerContext if available.
        if let Some(ctx) = self.context() {
            let wallet = ctx
                .wallets
                .lock()
                .expect("lock wallets")
                .get(self.seed())
                .cloned();
            if let Some(w) = wallet {
                let coins: Vec<ShieldedCoinBalance> = w
                    .shielded
                    .state
                    .coins
                    .iter()
                    .map(|(_nullifier, coin)| ShieldedCoinBalance {
                        token_type: hex::encode(coin.type_.into_inner().0),
                        value: coin.value,
                    })
                    .collect();
                let total_count = coins.len();
                return ShieldedBalance { coins, total_count };
            }
        }
        ShieldedBalance::default()
    }
}
