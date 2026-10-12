//! The shielded outputs and change that calls and deploys on this generation
//! share.

use std::collections::BTreeMap;
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

/// The attached value each key carries beyond what the outputs draw from it,
/// as `(key, change)`.
///
/// A key is what the ledger balances by: a token, or a token and a segment.
///
/// An output covered by a mint is funded by the ledger's mint credit rather
/// than by the attached coins, so only the uncovered remainder is charged to
/// them. Charging the whole output instead would leave the caller's coin
/// unaccounted, and the offer's balance would still be non-negative, so the
/// chain would accept the transaction and destroy it.
///
/// A key whose outputs draw more than was attached yields nothing here. That
/// shortfall makes the offer unbalanced, which the fee-paying step refuses
/// before submitting.
pub(super) fn change_due<K: Ord + Copy>(
    attached_value: &BTreeMap<K, u128>,
    drawn_value: &BTreeMap<K, u128>,
    minted_value: &BTreeMap<K, u128>,
) -> Vec<(K, u128)> {
    let mut change = Vec::new();
    for (key, attached) in attached_value {
        let drawn = drawn_value.get(key).copied().unwrap_or(0);
        let minted = minted_value.get(key).copied().unwrap_or(0);
        let Some(owed) = attached
            .checked_sub(drawn.saturating_sub(minted))
            .filter(|c| *c > 0)
        else {
            continue;
        };
        change.push((*key, owed));
    }
    change
}

#[cfg(test)]
mod tests {
    use super::*;
    use helpers::HashOutput;

    fn tt(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(HashOutput([byte; 32]))
    }

    /// The change output exists to return a surplus, so it must not appear when
    /// the attached coins fund the outputs exactly. An extra output there would
    /// push the offer's per-segment delta negative and the node would reject the
    /// transaction.
    #[test]
    fn change_is_empty_without_a_surplus() {
        let attached = BTreeMap::from([((tt(1), false), 10u128)]);
        let minted = BTreeMap::new();

        let exact = BTreeMap::from([((tt(1), false), 10u128)]);
        assert!(change_due(&attached, &exact, &minted).is_empty());

        // A shortfall is the node's to report, not ours to turn into an output.
        let over = BTreeMap::from([((tt(1), false), 11u128)]);
        assert!(change_due(&attached, &over, &minted).is_empty());
    }

    /// A surplus comes back in the segment its inputs rode in, and only the
    /// surplus. A token the outputs draw nothing of comes back whole.
    #[test]
    fn change_returns_the_surplus_per_token_and_segment() {
        let attached = BTreeMap::from([((tt(1), false), 10u128), ((tt(2), true), 7u128)]);
        let drawn = BTreeMap::from([((tt(1), false), 4u128)]);
        let minted = BTreeMap::new();

        let mut change = change_due(&attached, &drawn, &minted);
        change.sort_by_key(|((t, _), _)| t.0.0);
        assert_eq!(change, vec![((tt(1), false), 6), ((tt(2), true), 7)]);
    }

    /// A mint credit funds the output it covers, so charging that output to
    /// the attached coins would leave the caller's value unaccounted. The
    /// offer's balance stays non-negative either way, so the chain accepts the
    /// transaction and the difference is destroyed.
    #[test]
    fn change_does_not_charge_mint_funded_outputs_to_the_caller() {
        let attached = BTreeMap::from([((tt(1), false), 50u128)]);

        // Mints 100 and pays it straight out: the caller funds none of it, so
        // the whole attached coin comes back.
        let drawn = BTreeMap::from([((tt(1), false), 100u128)]);
        let minted = BTreeMap::from([((tt(1), false), 100u128)]);
        assert_eq!(
            change_due(&attached, &drawn, &minted),
            vec![((tt(1), false), 50)]
        );

        // Mints 100 and pays out 150: the caller funds the uncovered 50, so
        // nothing is owed back.
        let drawn = BTreeMap::from([((tt(1), false), 150u128)]);
        assert!(change_due(&attached, &drawn, &minted).is_empty());

        // Mints 100 and pays out 200: the caller is 50 short, which the
        // balancing step reports rather than this function.
        let drawn = BTreeMap::from([((tt(1), false), 200u128)]);
        assert!(change_due(&attached, &drawn, &minted).is_empty());
    }
}
