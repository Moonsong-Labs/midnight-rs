use midnight_types::TrackedUtxo;
use midnight_types::hd::Seed;

use super::helpers::LedgerParameters;
use super::state::Wallet;
use super::types::BuildInputs;

impl BuildInputs for Wallet {
    fn seed(&self) -> &Seed {
        Wallet::seed(self)
    }

    fn network(&self) -> &str {
        Wallet::network(self).as_str()
    }

    fn parameters(&self) -> &LedgerParameters {
        Wallet::parameters(self)
    }

    fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        Wallet::unshielded_utxos(self)
    }
}
