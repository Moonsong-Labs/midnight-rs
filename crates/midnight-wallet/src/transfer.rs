use midnight_helpers::LedgerParameters;
use midnight_types::hd::Seed;
use midnight_types::{BuildInputs, TrackedUtxo};

use super::state::Wallet;

impl BuildInputs for Wallet {
    fn seed(&self) -> &Seed {
        Wallet::seed(self)
    }

    fn parameters(&self) -> &LedgerParameters {
        Wallet::parameters(self)
    }

    fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        Wallet::unshielded_utxos(self)
    }
}
