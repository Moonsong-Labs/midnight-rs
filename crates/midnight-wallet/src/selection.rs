//! Which of a wallet's coins a build spends.
//!
//! Every shielded input carries its own proof: the client generates it, the
//! chain verifies it, and the fee is charged per input. Input count is
//! therefore the cost of a transaction, which is what makes selection a real
//! decision rather than an ordering preference.
//!
//! The upstream helper offers two orderings of one greedy accumulate, and
//! neither is a good default. Largest-first spends the fewest inputs but never
//! touches small coins, so it erodes the biggest coin and strands the rest.
//! Smallest-first absorbs those but maximises input count by construction, so a
//! fragmented wallet pays for many proofs on an ordinary payment. Selection
//! lives here instead, so the default can have both properties.

use crate::WalletError;

/// Upper bound on the inputs one build may draw.
///
/// Not a ledger rule. The chain's own limit is an overflow guard far above any
/// real transaction, and the byte limit allows a couple of hundred inputs. This
/// bounds client-side proving, which is seconds per input, so a wallet that has
/// fragmented into hundreds of coins fails fast and loudly rather than
/// appearing to hang. Deliberate consolidation past this runs as several
/// transfers.
pub const MAX_SELECTED_INPUTS: usize = 64;

/// Which coins a build draws on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CoinSelection {
    /// Spend the smallest single coin that covers the amount; when no single
    /// coin does, accumulate the largest ones until it is covered.
    ///
    /// One input in the common case, the least change of any single-coin
    /// choice, and it consumes smaller coins in preference to larger ones, so
    /// the wallet does not accumulate unspendable remainders. The fallback is
    /// largest-first because once several coins are needed, fewer is cheaper.
    #[default]
    BestFit,

    /// Spend the largest coins first. Fewest inputs for a payment that needs
    /// more than one coin, at the cost of never consuming small ones.
    LargestFirst,

    /// Spend the smallest coins first, which folds them into the payment and
    /// its change. The point is consolidation: expect many inputs, and expect
    /// to pay for each.
    SmallestFirst,
}

impl CoinSelection {
    /// The nearest upstream ordering, for inputs this module does not select.
    ///
    /// Unshielded UTXOs are authorised by one signature over the whole intent
    /// rather than a proof each, so input count barely moves their cost and the
    /// case for best fit does not apply. Those still go through the helpers'
    /// selector, which understands the ledger's UTXO set; best fit maps to
    /// fewest-inputs there.
    pub(crate) fn as_ordering(self) -> midnight_helpers::CoinSelectionStrategy {
        match self {
            Self::SmallestFirst => midnight_helpers::CoinSelectionStrategy::SmallestFirst,
            Self::BestFit | Self::LargestFirst => {
                midnight_helpers::CoinSelectionStrategy::LargestFirst
            }
        }
    }
}

/// One candidate coin: its value, and whatever the caller needs to identify it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate<T> {
    pub value: u128,
    pub id: T,
}

/// What a selection spends, and what comes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected<T> {
    pub coins: Vec<Candidate<T>>,
    /// Selected total minus the requested amount.
    pub change: u128,
}

/// Choose coins covering `required` from `candidates`.
///
/// `token` names the token in any error, since a caller holding several is
/// otherwise told only that something was short.
///
/// Selecting nothing for a `required` of zero is deliberate: the alternative is
/// spending a coin and handing it straight back, which costs two proofs and
/// achieves nothing.
pub fn select<T: Copy>(
    candidates: &[Candidate<T>],
    required: u128,
    token: &str,
    mode: CoinSelection,
) -> Result<Selected<T>, WalletError> {
    if required == 0 {
        return Ok(Selected {
            coins: Vec::new(),
            change: 0,
        });
    }

    let available: u128 = candidates.iter().map(|c| c.value).sum();
    if available < required {
        return Err(WalletError::InsufficientFunds {
            token: token.to_string(),
            required,
            available,
        });
    }

    if mode == CoinSelection::BestFit {
        // The smallest coin that covers the amount on its own: one input, and
        // the least change any single coin could leave.
        if let Some(best) = candidates
            .iter()
            .filter(|c| c.value >= required)
            .min_by_key(|c| c.value)
        {
            return Ok(Selected {
                coins: vec![*best],
                change: best.value - required,
            });
        }
    }

    let mut ordered: Vec<Candidate<T>> = candidates.to_vec();
    ordered.sort_by_key(|c| c.value);
    if mode != CoinSelection::SmallestFirst {
        // BestFit only reaches here when no single coin covers the amount, and
        // then fewer inputs is the cheaper answer, same as LargestFirst.
        ordered.reverse();
    }

    let mut coins = Vec::new();
    let mut total: u128 = 0;
    for candidate in ordered {
        total = total.saturating_add(candidate.value);
        coins.push(candidate);
        if coins.len() > MAX_SELECTED_INPUTS {
            return Err(WalletError::TooManyInputs {
                token: token.to_string(),
                max: MAX_SELECTED_INPUTS,
            });
        }
        if let Some(change) = total.checked_sub(required) {
            return Ok(Selected { coins, change });
        }
    }

    // `available >= required` was checked above, so the loop covers the amount
    // unless the total overflowed, which saturating_add would have masked.
    Err(WalletError::InsufficientFunds {
        token: token.to_string(),
        required,
        available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coins(values: &[u128]) -> Vec<Candidate<usize>> {
        values
            .iter()
            .enumerate()
            .map(|(id, &value)| Candidate { value, id })
            .collect()
    }

    fn values<T>(s: &Selected<T>) -> Vec<u128> {
        s.coins.iter().map(|c| c.value).collect()
    }

    /// The point of best fit: one input, and the smallest coin that does the
    /// job rather than the largest, so small coins get consumed instead of
    /// accumulating.
    #[test]
    fn best_fit_takes_the_smallest_sufficient_coin() {
        let c = coins(&[100, 7, 50, 9]);

        let s = select(&c, 5, "t", CoinSelection::BestFit).unwrap();
        assert_eq!(values(&s), vec![7]);
        assert_eq!(s.change, 2);

        let s = select(&c, 50, "t", CoinSelection::BestFit).unwrap();
        assert_eq!(values(&s), vec![50]);
        assert_eq!(s.change, 0);
    }

    /// With no single coin big enough, fewer inputs is cheaper, so it falls
    /// back to taking the largest.
    #[test]
    fn best_fit_falls_back_to_largest_first() {
        let s = select(&coins(&[10, 20, 30]), 45, "t", CoinSelection::BestFit).unwrap();
        assert_eq!(values(&s), vec![30, 20]);
        assert_eq!(s.change, 5);
    }

    /// Contrast with the explicit orderings, which never consider a single
    /// covering coin.
    #[test]
    fn explicit_orderings_accumulate_from_their_end() {
        let c = coins(&[10, 20, 30]);

        let s = select(&c, 25, "t", CoinSelection::LargestFirst).unwrap();
        assert_eq!(values(&s), vec![30]);

        let s = select(&c, 25, "t", CoinSelection::SmallestFirst).unwrap();
        assert_eq!(values(&s), vec![10, 20]);
        assert_eq!(s.change, 5);
    }

    /// Spending a coin to hand it straight back costs two proofs and changes
    /// nothing.
    #[test]
    fn zero_required_selects_nothing() {
        let s = select(&coins(&[10]), 0, "t", CoinSelection::BestFit).unwrap();
        assert!(s.coins.is_empty());
        assert_eq!(s.change, 0);
    }

    /// The error names the token and both sides of the gap, so a caller
    /// holding several tokens can act on it.
    #[test]
    fn insufficient_funds_reports_the_gap() {
        let err = select(&coins(&[10, 20]), 100, "tok", CoinSelection::BestFit).unwrap_err();
        match err {
            WalletError::InsufficientFunds {
                token,
                required,
                available,
            } => {
                assert_eq!(token, "tok");
                assert_eq!(required, 100);
                assert_eq!(available, 30);
            }
            other => panic!("expected InsufficientFunds, got {other:?}"),
        }
        assert!(select(&[] as &[Candidate<usize>], 1, "tok", CoinSelection::BestFit).is_err());
    }

    /// A wallet fragmented past the cap fails fast rather than generating
    /// minutes of proofs.
    #[test]
    fn too_many_inputs_is_refused() {
        let many = coins(&vec![1u128; MAX_SELECTED_INPUTS + 10]);
        let err = select(
            &many,
            (MAX_SELECTED_INPUTS + 5) as u128,
            "tok",
            CoinSelection::SmallestFirst,
        )
        .unwrap_err();
        assert!(
            matches!(err, WalletError::TooManyInputs { max, .. } if max == MAX_SELECTED_INPUTS),
            "got {err:?}"
        );

        // Exactly at the cap is still allowed.
        let s = select(
            &many,
            MAX_SELECTED_INPUTS as u128,
            "tok",
            CoinSelection::SmallestFirst,
        )
        .unwrap();
        assert_eq!(s.coins.len(), MAX_SELECTED_INPUTS);
    }
}
