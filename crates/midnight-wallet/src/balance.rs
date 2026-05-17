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

#[derive(Debug, Clone, Default)]
pub struct WalletBalance {
    pub dust: DustBalance,
    pub unshielded: Vec<UnshieldedUtxoInfo>,
}

impl WalletState {
    pub fn balance(&self) -> WalletBalance {
        WalletBalance {
            dust: self.dust_balance(),
            unshielded: self.unshielded_balance(),
        }
    }

    pub fn dust_balance(&self) -> DustBalance {
        DustBalance {
            spendable_utxos: self.dust_utxo_count(),
        }
    }

    pub fn unshielded_balance(&self) -> Vec<UnshieldedUtxoInfo> {
        self.unshielded_utxos()
            .into_iter()
            .map(|utxo| UnshieldedUtxoInfo {
                token_type: hex::encode(utxo.type_.into_inner().0),
                value: utxo.value,
            })
            .collect()
    }
}
