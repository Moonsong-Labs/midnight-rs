//! A shielded offer that a deploy carries.

use midnight_types::{ShieldedRecipient, ShieldedTokenType, SpendableShieldedCoin};

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
/// #     network: &midnight_types::Network,
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
///         recipient: parse_shielded_recipient(address, network)?,
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
    /// The deploy spends each coin whole. When the coins of a token hold less
    /// than [`Self::outputs`] pay of it, the wallet selects more of its coins
    /// of that token. What they hold beyond the payments goes back to the
    /// wallet as change.
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
