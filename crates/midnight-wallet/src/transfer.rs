use midnight_helpers::{DefaultDB, DustWallet, LedgerParameters, WalletSeed};
pub use midnight_types::transfer::*;

use crate::state::{TrackedUtxo, Wallet};

impl BuildInputs for Wallet {
    fn seed(&self) -> &WalletSeed {
        Wallet::seed(self)
    }

    fn network(&self) -> &str {
        Wallet::network(self)
    }

    fn parameters(&self) -> &LedgerParameters {
        Wallet::parameters(self)
    }

    fn dust_wallet(&self) -> &DustWallet<DefaultDB> {
        Wallet::dust_wallet(self)
    }

    fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        Wallet::unshielded_utxos(self)
    }
}
