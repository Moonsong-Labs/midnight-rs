//! The keys that a synced wallet holds in place of its seed.

use midnight_helpers::{
    DefaultDB, DustWallet, IntoWalletAddress, SecretKeys, ShieldedWallet, UnshieldedWallet,
    ledger_8,
};
use midnight_types::convert::IntoLedger;

use crate::{Network, Seed};

/// The keys of each role, which a sync derives from the seed. The seed then
/// drops.
///
/// The wallet keeps them for its whole life: the shielded and Dust spends
/// prove with their secret keys, and the unshielded key signs.
pub(crate) struct RoleKeys {
    /// The Zswap keys of ledger 9.
    pub(crate) zswap: SecretKeys,
    /// The Zswap keys of ledger 8, which read the shielded events that a chain
    /// made before its hard fork to ledger 9.
    pub(crate) zswap_pre_fork: ledger_8::SecretKeys,
    /// The unshielded keys.
    pub(crate) unshielded: UnshieldedWallet,
    /// The Dust keys, with no local state.
    pub(crate) dust: DustWallet<DefaultDB>,
}

impl RoleKeys {
    /// The keys of the default path of each role of `seed`.
    pub(crate) fn derive(seed: &Seed) -> Self {
        let root = seed.into_ledger();
        let pre_fork_root = ledger_8::WalletSeed::try_from(seed.as_bytes())
            .expect("a seed holds 16, 32 or 64 bytes, which ledger 8 takes");
        Self {
            zswap: ShieldedWallet::<DefaultDB>::default(root.clone())
                .secret_keys()
                .clone(),
            zswap_pre_fork: ledger_8::ShieldedWallet::<DefaultDB>::default(pre_fork_root)
                .secret_keys()
                .clone(),
            unshielded: UnshieldedWallet::default(root.clone()),
            dust: DustWallet::default(root, None),
        }
    }

    /// The unshielded address on `network`.
    pub(crate) fn unshielded_address(&self, network: &Network) -> String {
        self.unshielded.address(network.as_str()).to_bech32()
    }

    /// The shielded address on `network`.
    pub(crate) fn shielded_address(&self, network: &Network) -> String {
        ShieldedWallet::<DefaultDB>::from_pub_keys(
            self.zswap.coin_public_key(),
            self.zswap.enc_public_key(),
        )
        .address(network.as_str())
        .to_bech32()
    }

    /// The Dust address on `network`.
    pub(crate) fn dust_address(&self, network: &Network) -> String {
        self.dust.address(network.as_str()).to_bech32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unshielded address names the storage directory of the wallet, and
    /// all three are what a user shares. An address that the keys give and
    /// the seed does not would send funds to a key that the wallet never
    /// tracks, or start an empty snapshot beside the real one.
    #[test]
    fn the_keys_give_the_addresses_of_their_seed() {
        let seed = Seed::from_bytes(&[0x42; 32]).unwrap();
        let keys = RoleKeys::derive(&seed);
        let network = Network::Undeployed;

        assert_eq!(
            keys.unshielded_address(&network),
            seed.unshielded_address(&network)
        );
        assert_eq!(
            keys.shielded_address(&network),
            seed.shielded_address(&network)
        );
        assert_eq!(keys.dust_address(&network), seed.dust_address(&network));
    }
}
