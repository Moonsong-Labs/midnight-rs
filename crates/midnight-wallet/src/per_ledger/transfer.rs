use midnight_types::{Seed, TrackedUtxo};

use super::helpers::{DefaultDB, DustWallet, LedgerParameters};
use super::state::Wallet;
use super::types::BuildInputs;

impl BuildInputs for Wallet {
    fn seed(&self) -> &Seed {
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
