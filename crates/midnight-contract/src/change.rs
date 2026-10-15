//! The shielded change that calls and deploys owe back to the wallet.

use std::collections::BTreeMap;

use crate::error::ContractError;

/// The total value under each key in `values`.
///
/// A key is what the ledger balances by: a token, or a token and a segment.
pub(crate) fn total_by_key<K: Ord>(
    values: impl IntoIterator<Item = (K, u128)>,
) -> Result<BTreeMap<K, u128>, ContractError> {
    let mut totals = BTreeMap::new();
    for (key, value) in values {
        let total: &mut u128 = totals.entry(key).or_default();
        *total = total.checked_add(value).ok_or_else(|| {
            ContractError::Construction("shielded values overflow a u128 total".into())
        })?;
    }
    Ok(totals)
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
pub(crate) fn change_due<K: Ord + Copy>(
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
    use midnight_types::{HashOutput, ShieldedTokenType};

    fn tt(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(HashOutput([byte; 32]))
    }

    /// A total that wraps or saturates would size the change from a value the
    /// coins do not hold.
    #[test]
    fn a_total_past_u128_is_refused() {
        let totals = total_by_key([(tt(1), u128::MAX), (tt(2), 1)]).expect("separate keys");
        assert_eq!(totals, BTreeMap::from([(tt(1), u128::MAX), (tt(2), 1)]));

        let err = total_by_key([(tt(1), u128::MAX), (tt(1), 1)]).unwrap_err();
        assert!(matches!(err, ContractError::Construction(_)), "got {err:?}");
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
