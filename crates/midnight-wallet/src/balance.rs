use midnight_node_ledger_helpers::Timestamp;

use crate::state::WalletState;

#[derive(Debug, Clone, Default)]
pub struct DustBalance {
    pub spendable_utxos: usize,
    /// Current dust balance in SPECK (1 DUST = 10^15 SPECK).
    /// Computed at the time of the balance query using UTXO age and generation parameters.
    pub balance_speck: u128,
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
            .map(|utxo| UnshieldedUtxoInfo {
                token_type: utxo.token_type.clone(),
                value: utxo.value,
            })
            .collect()
    }

    pub fn shielded_balance(&self) -> ShieldedBalance {
        let coins: Vec<ShieldedCoinBalance> = self
            .zswap_state()
            .coins
            .iter()
            .map(|(_nullifier, coin)| ShieldedCoinBalance {
                token_type: hex::encode(coin.type_.into_inner().0),
                value: coin.value,
            })
            .collect();
        let total_count = coins.len();
        ShieldedBalance { coins, total_count }
    }
}
