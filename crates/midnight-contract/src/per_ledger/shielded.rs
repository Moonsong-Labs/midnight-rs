//! The shielded outputs that calls and deploys on this generation share.

use std::sync::Arc;

use super::helpers;
use helpers::ShieldedTokenType;
use helpers::coin_structure::coin::Info as ZswapCoinInfo;

/// A fresh shielded coin for the holder of two public keys.
///
/// The output carries a discovery ciphertext sealed to `enc_public_key`, so
/// the recipient's wallet finds the coin through normal sync.
///
/// Addressing a coin needs only public material, so this holds the two public
/// keys rather than the seed the helpers' `OutputInfo` takes. That keeps the
/// type usable by a signer that never releases its seed, and it is why change
/// is discovered through its ciphertext rather than registered with
/// `watch_for`, which would need the wallet behind the seed.
pub(super) struct KeyedOutput {
    pub(super) coin_public_key: helpers::CoinPublicKey,
    pub(super) enc_public_key: helpers::EncryptionPublicKey,
    pub(super) token_type: ShieldedTokenType,
    pub(super) value: u128,
}

impl helpers::TokenInfo for KeyedOutput {
    fn token_type(&self) -> ShieldedTokenType {
        self.token_type
    }
    fn value(&self) -> u128 {
        self.value
    }
}

impl helpers::BuildOutput<helpers::DefaultDB, helpers::BuildContext> for KeyedOutput {
    fn build(
        &self,
        rng: &mut helpers::StdRng,
        _context: Arc<helpers::BuildContext>,
    ) -> helpers::Output<helpers::ProofPreimage, helpers::DefaultDB> {
        let coin = ZswapCoinInfo::new(rng, self.value, self.token_type);
        helpers::Output::new(
            rng,
            &coin,
            helpers::Segment::Guaranteed.into(),
            &self.coin_public_key,
            Some(self.enc_public_key),
        )
        .expect("a coin addressed to public keys must be constructible")
    }
}
