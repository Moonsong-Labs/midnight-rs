//! Shielded inputs produced by the wallet, not by the offer builder.
//!
//! Spending a coin proves ownership, so it needs the spending keys and no
//! public-material substitute exists. What can change is where those keys are
//! read. The helpers' `InputInfo<WalletSeed>` carries a seed into the offer and
//! resolves it during `build`, so the builder holds a secret for the lifetime of
//! the transaction. Here the wallet performs the spend up front and the builder
//! receives a finished `Input`, which carries none.
//!
//! That is also the shape an external signer needs: "give me an `Input` for this
//! coin" is one step it can implement without releasing anything.

use std::sync::Arc;

use midnight_helpers::{
    DefaultDB, Input, LedgerContext, Nullifier, ProofPreimage, Segment, ShieldedTokenType, StdRng,
    TokenInfo, WalletSeed,
};

use crate::WalletError;

/// A shielded input the wallet already produced.
///
/// `build` hands it over and ignores the rng and context it is passed: the
/// spend has happened, and repeating it would produce a different nullifier for
/// a coin already committed to this transaction.
pub struct PreparedInput {
    input: Option<Input<ProofPreimage, DefaultDB>>,
    token_type: ShieldedTokenType,
    value: u128,
}

impl TokenInfo for PreparedInput {
    fn token_type(&self) -> ShieldedTokenType {
        self.token_type
    }

    /// The value the spent coin carries, read from the wallet and fixed before
    /// the offer sums it.
    ///
    /// The offer's declared delta is built from this, and the ledger checks
    /// that delta against the commitments the spend proofs carry, so a figure
    /// that did not come from the coin would make the transaction malformed.
    /// The helpers' input instead rewrites this during `build` with whichever
    /// coin it resolved, which is how a request for less than a coin's value
    /// used to enter the offer as the whole coin.
    fn value(&self) -> u128 {
        self.value
    }
}

impl midnight_helpers::BuildInput<DefaultDB> for PreparedInput {
    fn build(
        &mut self,
        _rng: &mut StdRng,
        _context: Arc<LedgerContext<DefaultDB>>,
    ) -> Input<ProofPreimage, DefaultDB> {
        self.input
            .take()
            .expect("a prepared input is built once; the offer builder consumes each input once")
    }
}

/// Spend the named coins from `seed`'s wallet, returning inputs the offer
/// builder can hold without a secret.
///
/// A coin is named only by its nullifier. Its token and value come from the
/// wallet, never from the caller, so the figure the offer sums is the one the
/// spend proof commits to and the two cannot drift apart. The wallet's Zswap
/// state is rolled forward as it goes, the same bookkeeping the helpers' input
/// does inside `build`, so a coin cannot be spent twice within one offer.
pub fn prepare_shielded_inputs(
    context: &Arc<LedgerContext<DefaultDB>>,
    seed: &WalletSeed,
    nullifiers: &[Nullifier],
    rng: &mut StdRng,
) -> Result<Vec<PreparedInput>, WalletError> {
    context.with_wallet_from_seed(seed.clone(), |wallet| {
        let mut prepared = Vec::with_capacity(nullifiers.len());
        for nullifier in nullifiers {
            let coin = wallet.shielded.state.coins.get(nullifier).ok_or_else(|| {
                // Deliberately not naming the wallet: the upstream selector
                // panics here with the whole state in the message.
                WalletError::Transfer(format!(
                    "shielded coin {nullifier:?} is not spendable by this wallet \
                         (already spent, not yet synced, or not owned by it)"
                ))
            })?;
            // Read before the spend, which replaces the state this borrows.
            let (token_type, value) = (coin.type_, coin.value);

            let (updated, input) = wallet
                .shielded
                .state
                .spend(
                    rng,
                    wallet.shielded.secret_keys(),
                    coin,
                    Segment::Guaranteed.into(),
                )
                .map_err(|e| WalletError::Transfer(format!("spend shielded coin: {e:?}")))?;
            wallet.shielded.state = updated;

            prepared.push(PreparedInput {
                input: Some(input),
                token_type,
                value,
            });
        }
        Ok(prepared)
    })
}
