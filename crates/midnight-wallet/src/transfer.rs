use midnight_types::build::{BuildInputs, TrackedUtxo};
use midnight_types::hd::Seed;

use super::state::Wallet;

impl BuildInputs for Wallet {
    fn seed(&self) -> &Seed {
        Wallet::seed(self)
    }

    fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        Wallet::unshielded_utxos(self)
    }
}
