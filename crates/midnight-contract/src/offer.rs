//! A shielded offer that a deploy carries.

use std::collections::BTreeMap;

use midnight_types::{ShieldedRecipient, ShieldedTokenType, SpendableShieldedCoin};

use crate::error::ContractError;

/// Shielded coins that a deploy spends, and the shielded payments it makes
/// from them.
///
/// Attach one with
/// [`DeployBuilder::with_shielded_offer`](crate::DeployBuilder::with_shielded_offer),
/// which describes how the deploy builds it.
///
/// # Example
///
/// ```rust,no_run
/// use midnight_contract::{ShieldedOffer, ShieldedPayment, ShieldedTokenType};
/// use midnight_contract::parse_shielded_recipient;
///
/// # async fn offer(
/// #     provider: &midnight_provider::MidnightProvider,
/// #     token_type: ShieldedTokenType,
/// #     address: &str,
/// # ) -> Result<ShieldedOffer, Box<dyn std::error::Error>> {
/// let coin = provider
///     .spendable_shielded_coins()
///     .await?
///     .into_iter()
///     .find(|coin| coin.token_type == token_type && coin.value >= 10)
///     .ok_or_else(|| std::io::Error::other("no coin of the token holds 10"))?;
/// let offer = ShieldedOffer {
///     coins: vec![coin],
///     outputs: vec![ShieldedPayment {
///         recipient: parse_shielded_recipient(address, "undeployed")?,
///         token_type,
///         value: 10,
///     }],
/// };
/// # Ok(offer)
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ShieldedOffer {
    /// The attached wallet's coins to spend, from
    /// [`MidnightProvider::spendable_shielded_coins`](midnight_provider::MidnightProvider::spendable_shielded_coins).
    ///
    /// The deploy spends each coin whole. What the coins of a token hold
    /// beyond what [`Self::outputs`] pay of it goes back to the wallet as
    /// change.
    pub coins: Vec<SpendableShieldedCoin>,
    /// The payments to make from the coins.
    pub outputs: Vec<ShieldedPayment>,
}

/// A payment of a shielded token to the holder of a pair of shielded keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShieldedPayment {
    /// The keys that receive the coin.
    /// [`parse_shielded_recipient`](crate::parse_shielded_recipient) reads
    /// them from a shielded address.
    pub recipient: ShieldedRecipient,
    /// The token to pay.
    pub token_type: ShieldedTokenType,
    /// The amount to pay, in the token's smallest unit.
    pub value: u128,
}

/// The total value of each token in `values`.
pub(crate) fn value_by_token(
    values: impl IntoIterator<Item = (ShieldedTokenType, u128)>,
) -> Result<BTreeMap<ShieldedTokenType, u128>, ContractError> {
    let mut totals = BTreeMap::new();
    for (token_type, value) in values {
        let total: &mut u128 = totals.entry(token_type).or_default();
        *total = total.checked_add(value).ok_or_else(|| {
            ContractError::Construction("shielded offer values overflow a u128 total".into())
        })?;
    }
    Ok(totals)
}

/// Refuse an offer whose outputs pay more of a token than its coins hold.
///
/// `held` and `paid` are the totals of each token. The check walks the paid
/// tokens, so an output of a token that no coin holds is a shortfall too.
pub(crate) fn refuse_shortfall(
    held: &BTreeMap<ShieldedTokenType, u128>,
    paid: &BTreeMap<ShieldedTokenType, u128>,
) -> Result<(), ContractError> {
    for (token_type, paid) in paid {
        let held = held.get(token_type).copied().unwrap_or(0);
        if *paid > held {
            return Err(ContractError::ShieldedOfferShortfall {
                token_type: *token_type,
                held,
                paid: *paid,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_types::HashOutput;

    fn tt(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(HashOutput([byte; 32]))
    }

    /// A check that walks the coins' tokens never sees a paid token that no
    /// coin holds, and the fee step then refuses the deploy without naming it.
    #[test]
    fn a_shortfall_is_refused_with_the_token_and_amounts() {
        let held = BTreeMap::from([(tt(1), 10)]);

        assert!(
            refuse_shortfall(&held, &BTreeMap::from([(tt(1), 10)])).is_ok(),
            "coins that hold exactly what the outputs pay cover them"
        );

        let over = refuse_shortfall(&held, &BTreeMap::from([(tt(1), 11)])).unwrap_err();
        assert!(
            matches!(
                over,
                ContractError::ShieldedOfferShortfall { token_type, held: 10, paid: 11 }
                    if token_type == tt(1)
            ),
            "got {over:?}"
        );

        let unheld =
            refuse_shortfall(&held, &BTreeMap::from([(tt(1), 1), (tt(2), 1)])).unwrap_err();
        assert!(
            matches!(
                unheld,
                ContractError::ShieldedOfferShortfall { token_type, held: 0, paid: 1 }
                    if token_type == tt(2)
            ),
            "got {unheld:?}"
        );
    }
}
