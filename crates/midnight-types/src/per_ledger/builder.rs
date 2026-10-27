//! A transfer build: select the inputs, balance the Dust fee, and prove.
//!
//! A wallet implementation selects inputs and assembles a [`PreparedTransfer`]
//! with a [`TransferBuilder`]; proving it ([`PreparedTransfer::prove`]) needs
//! the build context alone.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use futures_util::FutureExt;

use super::convert::{IntoLedger, IntoSdk, sdk_seed};
use super::helpers;
use helpers::{
    BuildContext, BuildUtxoOutput, BuildUtxoSpend, BuilderContext, CoinSelectionStrategy,
    DefaultDB, DustActions, DustLocalState, DustRegistration, DustRegistrationBuilder, DustSpend,
    DustWallet, FromContext, HashMapStorage, InputInfo, Intent, IntentInfo, LedgerParameters,
    NIGHT, Nullifier, OfferInfo, OutputInfo, ProofPreimageMarker, ProofProvider, Segment,
    ShieldedCoinSelectionError, ShieldedTokenType, ShieldedWallet, Signature, Sp, SplittableRng,
    StandardTransactionInfo, StdRng, Timestamp, TokenType, Transaction, TransactionSigningKey,
    UnshieldedOffer, UnshieldedOfferInfo, UnshieldedTokenType, UnshieldedWallet, UtxoOutputInfo,
    UtxoSelectionError, UtxoSpendInfo, WalletAddress,
};

use crate::hd::Seed;
use crate::sync::TrackedUtxo;
use crate::{
    Network, ReservedInput, SpentInputs, SpentUtxoKey, TransferKind, TransferRequest,
    TransferResult, WalletError, panic_message,
};

type UnprovenTx = helpers::UnprovenTransaction<DefaultDB>;
type FinalizedTx = helpers::FinalizedTransaction<DefaultDB>;

/// One funding seed's Dust contribution to a transaction.
///
/// `speculative_spend` returns the spend records and the resulting
/// `DustLocalState`, and `DustWallet::mark_spent` takes that pair together.
/// A batch holds the pair from one `speculative_spend`, and a build applies
/// each batch with one `mark_spent` call.
#[derive(Clone)]
pub struct DustSpendBatch {
    /// The seed whose Dust funds these spends.
    pub seed: Seed,
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    pub updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB>,
}

/// A transfer whose inputs are selected and whose Dust fee is balanced, but
/// which is not yet proven.
///
/// Reserve what [`Self::spent_inputs`] reports before you release the wallet.
/// Proving reads the build context alone, so [`Self::prove`] can run after the
/// release.
pub struct PreparedTransfer {
    tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: UnprovenTx,
    dust_batches: Vec<DustSpendBatch>,
    spent_unshielded_inputs: Vec<SpentUtxoKey>,
    spent_shielded_inputs: Vec<Nullifier>,
    /// The chain time this build selected against, taken from the context it
    /// was handed. The reservation carries the same stamp, so a release can
    /// name this build's own entry.
    reserved_at: Timestamp,
    /// The finished transaction this build balances, which [`Self::prove`]
    /// merges the proven build into.
    external: Option<FinalizedTx>,
}

impl PreparedTransfer {
    /// Wrap an assembled, fee-balanced, unproven transaction.
    ///
    /// For the build paths that produce one; the spends they report are added
    /// with the two setters below.
    pub fn new(
        tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        tx: UnprovenTx,
        dust_batches: Vec<DustSpendBatch>,
    ) -> Self {
        // Read the stamp from the context this build selected against rather
        // than take it as an argument, so the two cannot disagree.
        let reserved_at = tx_info.context.latest_block_context().tblock;
        Self {
            tx_info,
            tx,
            dust_batches,
            spent_unshielded_inputs: Vec::new(),
            spent_shielded_inputs: Vec::new(),
            reserved_at,
            external: None,
        }
    }

    /// Record the unshielded UTXOs this build spends.
    pub fn set_spent_unshielded_inputs(&mut self, inputs: Vec<SpentUtxoKey>) {
        self.spent_unshielded_inputs = inputs;
    }

    /// Record the shielded coins this build spends, by nullifier.
    pub fn set_spent_shielded_inputs(&mut self, inputs: Vec<Nullifier>) {
        self.spent_shielded_inputs = inputs;
    }
    /// The Dust batches that will fund this build's fee.
    pub fn dust_batches(&self) -> &[DustSpendBatch] {
        &self.dust_batches
    }

    /// The unshielded UTXOs this build will spend.
    pub fn spent_unshielded_inputs(&self) -> &[SpentUtxoKey] {
        &self.spent_unshielded_inputs
    }

    /// The shielded coins this build will spend, by nullifier.
    pub fn spent_shielded_inputs(&self) -> &[Nullifier] {
        &self.spent_shielded_inputs
    }

    /// The chain time this build selected against. Reserve its inputs with
    /// this stamp, which [`Self::spent_inputs`] gives each of them.
    pub fn reserved_at(&self) -> Timestamp {
        self.reserved_at
    }

    /// Everything this build will spend, in the form a reservation takes.
    pub fn spent_inputs(&self) -> SpentInputs {
        SpentInputs {
            dust: self.stamped(dust_nullifiers(&self.dust_batches)),
            unshielded: self.stamped(self.spent_unshielded_inputs.iter().cloned()),
            shielded: self.stamped(self.spent_shielded_inputs.iter().map(|n| n.into_sdk())),
        }
    }

    fn stamped<T>(&self, inputs: impl IntoIterator<Item = T>) -> Vec<ReservedInput<T>> {
        inputs
            .into_iter()
            .map(|input| ReservedInput {
                input,
                reserved_at: self.reserved_at,
            })
            .collect()
    }

    /// Prove the transaction and serialize it. The slowest step in a build,
    /// and it touches no wallet.
    ///
    /// A build from [`balance_external`] or [`balance_external_tokens`]
    /// proves what it adds and merges that into the transaction it balances.
    /// The result then holds the merged transaction and its fee, and its
    /// `spent` names only the inputs of this build.
    pub async fn prove(self) -> Result<TransferResult, WalletError> {
        let spent = self.spent_inputs();
        let PreparedTransfer {
            mut tx_info,
            tx,
            external,
            ..
        } = self;

        // Keep a handle to the ledger context so we can read `parameters`
        // after proving to compute the fee. `Arc::clone` is cheap and the lock
        // inside `with_ledger_state` is held only for the closure.
        let context = tx_info.context.clone();
        let proven = prove_tx_no_validate(&mut tx_info, tx).await?;
        let finalized = match external {
            Some(external) => external
                .merge(&proven)
                .map_err(transfer_err("merge fee intent"))?,
            None => proven,
        };

        let mut bytes = Vec::new();
        helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
            .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;
        let fee_speck = fee_speck(&context, &finalized)?;

        Ok(TransferResult {
            tx_bytes: bytes,
            ledger_version: super::LEDGER,
            spent,
            fee_speck,
        })
    }
}

/// The Dust fee, in SPECK, that the chain charges for `tx`, against the
/// ledger parameters in `context`. See [`TransferResult::fee_speck`].
///
/// # Errors
///
/// [`WalletError::Transfer`] when the ledger cannot price `tx`.
pub fn fee_speck(context: &BuildContext, tx: &FinalizedTx) -> Result<u128, WalletError> {
    // Mirrors the node's own estimation RPC: `enforce_time_to_dismiss =
    // false`, i.e. report the deterministic SPECK cost without the
    // chain-side mempool-eviction check. The chain charges the same number
    // at inclusion; if the tx exceeds the eviction-time bound, that
    // surfaces at submit, not here.
    context
        .with_ledger_state(|s| tx.fees(&s.parameters, false))
        .map_err(transfer_err("fees"))
}

/// The nullifier of every Dust spend in `batches`, which is how a release
/// names them.
fn dust_nullifiers(batches: &[DustSpendBatch]) -> impl Iterator<Item = crate::DustNullifier> {
    batches
        .iter()
        .flat_map(|b| b.spends.iter().map(|s| s.old_nullifier.into_sdk()))
}

fn flat_spends(batches: &[DustSpendBatch]) -> Vec<DustSpend<ProofPreimageMarker, DefaultDB>> {
    batches
        .iter()
        .flat_map(|b| b.spends.iter().cloned())
        .collect()
}

/// Wrap a foreign error as [`WalletError::Transfer`], naming the failing
/// step.
fn transfer_err<E: std::fmt::Debug>(ctx: &str) -> impl FnOnce(E) -> WalletError + '_ {
    move |e| WalletError::Transfer(format!("{ctx}: {e:?}"))
}

/// Type a shielded coin selection that fell short, with what the seed holds of
/// the token in the funding view the selection read.
fn shielded_selection_error(context: &BuildContext, e: ShieldedCoinSelectionError) -> WalletError {
    match e {
        ShieldedCoinSelectionError::InsufficientBalance {
            required,
            token_type,
            seed,
        } => {
            let available = context.with_wallet_from_seed(seed, |wallet| {
                wallet
                    .shielded
                    .state
                    .coins
                    .iter()
                    .filter(|(_, coin)| coin.type_ == token_type)
                    .fold(0u128, |sum, (_, coin)| sum.saturating_add(coin.value))
            });
            WalletError::InsufficientShielded {
                token_type: token_type.into_sdk(),
                required,
                available,
            }
        }
        e => WalletError::Transfer(format!("shielded coin selection: {e}")),
    }
}

/// Type an unshielded UTXO selection that fell short, with what the seed holds
/// of the token in the funding view the selection read.
async fn utxo_selection_error(context: &BuildContext, e: UtxoSelectionError) -> WalletError {
    match e {
        UtxoSelectionError::InsufficientBalance {
            required,
            token_type,
            seed,
        } => {
            let available = context
                .unshielded_utxos(seed)
                .await
                .into_iter()
                .filter(|(utxo, _)| utxo.type_ == token_type)
                .fold(0u128, |sum, (utxo, _)| sum.saturating_add(utxo.value));
            WalletError::InsufficientUnshielded {
                token_type: token_type.into_sdk(),
                required,
                available,
            }
        }
        e => WalletError::Transfer(format!("utxo selection: {e}")),
    }
}

/// The nullifier of each coin a selection picked, in the form a reservation
/// takes.
///
/// Every input from `coins_to_cover_value` carries a pinned nullifier. A
/// missing one is an error rather than a skipped coin, because a coin left
/// unreserved defeats the double-spend protection.
fn pinned_nullifiers<O>(inputs: &[InputInfo<O>]) -> Result<Vec<Nullifier>, WalletError> {
    inputs
        .iter()
        .map(|i| {
            i.nullifier.ok_or_else(|| {
                WalletError::Transfer("selected shielded coin has no nullifier to reserve".into())
            })
        })
        .collect()
}

/// The key of each UTXO a selection picked, in the form a reservation takes.
fn spent_utxo_keys<O>(spends: &[UtxoSpendInfo<O>]) -> Vec<SpentUtxoKey> {
    spends
        .iter()
        .filter_map(|s| {
            let intent_hash = s.intent_hash.as_ref()?;
            let output_index = s.output_number?;
            Some(SpentUtxoKey {
                intent_hash: intent_hash.0,
                output_index,
            })
        })
        .collect()
}

pub async fn prove_tx_no_validate(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: UnprovenTx,
) -> Result<FinalizedTx, WalletError> {
    // The trait accessor: the inherent one lends the resolver only for the
    // context's borrow, and `ProofProvider::prove` takes it `'static`.
    let resolver = BuilderContext::resolver(&*tx_info.context).await;
    let cost_model = tx_info
        .context
        .with_ledger_state(|ls| ls.parameters.cost_model.runtime_cost_model.clone());
    let mut rng = tx_info.rng.split();
    // `ProofProvider::prove` returns a bare transaction, so a backend that
    // fails has nowhere to report it but the unwind. Catch it here and hand
    // the caller a typed error instead of tearing down their task.
    let proven =
        std::panic::AssertUnwindSafe(tx_info.prover.prove(tx, rng.split(), resolver, cost_model))
            .catch_unwind()
            .await
            .map_err(|payload| WalletError::Proving(panic_message(payload)))?;
    Ok(proven.seal(rng))
}

/// What a transfer build reads from the wallet that funds it.
///
/// A build signs with the seed and checks a recipient address against the
/// network. A dust registration also reads the ledger parameters and the
/// tNIGHT UTXOs to choose one from. Everything else a build needs is in its
/// [`BuildContext`]. These readings are the whole of the wallet that a build
/// depends on.
///
/// A dust registration names the Dust public key of the seed's default Dust
/// path (`m/44'/2400'/0'/2/0`). A wallet that tracks its Dust under another
/// key does not see the Dust that the registration generates.
pub trait BuildInputs: Send + Sync {
    /// The seed the build signs and derives with.
    fn seed(&self) -> &Seed;

    /// The network a recipient address must belong to.
    fn network(&self) -> &Network;

    /// The ledger parameters a dust registration computes its fee allowance
    /// from.
    fn parameters(&self) -> &LedgerParameters;

    /// The unshielded UTXOs a dust registration picks its tNIGHT from.
    fn unshielded_utxos(&self) -> &[TrackedUtxo];
}

pub struct TransferBuilder<'a> {
    state: &'a dyn BuildInputs,
    context: Arc<BuildContext>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    coin_selection: CoinSelectionStrategy,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(
        state: &'a dyn BuildInputs,
        context: Arc<BuildContext>,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            state,
            context,
            proof_provider,
            coin_selection: CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on.
    ///
    /// [`CoinSelectionStrategy::LargestFirst`], the default, spends the fewest
    /// inputs, which matters because every shielded input carries its own proof
    /// and is charged for separately. [`CoinSelectionStrategy::SmallestFirst`]
    /// spends the most, absorbing small coins that the default would leave
    /// untouched indefinitely, at the cost of a larger and more expensive
    /// transaction. Nothing caps how many inputs the latter draws, so prefer it
    /// for a deliberate cleanup rather than for ordinary payments.
    ///
    /// Neither ordering is a good default, since one never absorbs small coins
    /// and the other is unbounded. TODO: replace both with best-fit selection,
    /// <https://github.com/Moonsong-Labs/midnight-rs/issues/145>
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Prepare the transfer a request names, with the input ordering it asks
    /// for. Dispatches to the matching `prepare_*` method below, each of which
    /// documents what it builds.
    pub async fn prepare(self, request: TransferRequest) -> Result<PreparedTransfer, WalletError> {
        let TransferRequest {
            kind,
            coin_selection,
        } = request;
        let builder = self.with_coin_selection(coin_selection);
        match kind {
            TransferKind::Shielded {
                token_type,
                amount,
                recipient,
                pay_fees,
            } => {
                builder
                    .prepare_shielded(token_type.into_ledger(), amount, &recipient, pay_fees)
                    .await
            }
            TransferKind::Unshielded {
                token_type,
                amount,
                recipient,
                pay_fees,
            } => {
                builder
                    .prepare_unshielded(token_type.into_ledger(), amount, &recipient, pay_fees)
                    .await
            }
            TransferKind::ShieldedSwap {
                give_token,
                give_amount,
                receive_token,
                receive_amount,
            } => {
                builder
                    .prepare_shielded_swap(
                        give_token.into_ledger(),
                        give_amount,
                        receive_token.into_ledger(),
                        receive_amount,
                    )
                    .await
            }
            TransferKind::DustRegistration { utxo_ctime } => {
                builder.prepare_register_dust(utxo_ctime).await
            }
        }
    }

    /// Build a shielded (ZSwap) transfer transaction.
    ///
    /// `recipient` is a bech32 shielded address (e.g.
    /// `mn_shield-addr_undeployed1...`). Only the public material is needed:
    /// the address carries the recipient's `coin_public_key` and
    /// `enc_public_key`, which is all the chain needs to construct the output
    /// coin commitment and encrypt the coin info for them.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for a multi-party flow where another
    /// wallet pays the fees (the `.without_dust()` path). It is not submittable
    /// on its own; hand it to the fee payer, who completes it with
    /// `MidnightProvider::balance_transaction` (in `midnight-provider`) and
    /// submits.
    ///
    /// `amount` may span several coins: inputs are selected up front with
    /// [`InputInfo::coins_to_cover_value`] and the remainder returns to this
    /// wallet as a change output, so a payment is bounded by the wallet's total
    /// balance in `token_type` rather than by its largest single coin. Sending
    /// the full balance to this wallet's own address therefore consolidates it
    /// into one coin, and sending part of a coin splits it. The spent coins'
    /// nullifiers are in [`PreparedTransfer::spent_shielded_inputs`].
    pub async fn prepare_shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let recipient_wallet = shielded_destination(recipient, self.state.network())?;
        let payment = OutputInfo {
            destination: recipient_wallet,
            token_type,
            value: amount,
        };
        self.prepare_shielded_offer(token_type, amount, Box::new(payment), pay_fees)
            .await
    }

    /// Build one half of a native two-party shielded token swap: a fee-less
    /// Zswap offer that spends `give_amount` of `give_token` and
    /// creates an output for `receive_amount` of `receive_token` payable to
    /// this wallet.
    ///
    /// The half is intentionally unbalanced: its offer deltas are
    /// `give_token = +give_amount` (surplus given up) and
    /// `receive_token = -receive_amount` (deficit to be filled). It cannot
    /// stand alone. The counterparty builds the exact mirror
    /// (`shielded_swap(receive_token, receive_amount, give_token, give_amount)`),
    /// and merging the two cancels both tokens into a balanced transaction that
    /// a sponsor funds (`balance_transaction`) and submits. Or a payer
    /// completes the half alone with `balance_transaction_with_tokens` (both
    /// on `MidnightProvider` in `midnight-provider`).
    ///
    /// Because the half is unbalanced it can't self-fund its Dust, so it is
    /// always fee-less: no funding seed is set and the returned transaction is
    /// inherently Dustless. Give-side coins are selected up front with
    /// [`InputInfo::coins_to_cover_value`]; the resulting change (if any) goes
    /// back to this wallet, mirroring [`Self::prepare_unshielded`]'s change handling in
    /// the shielded domain. The spent coins' nullifiers are in
    /// [`PreparedTransfer::spent_shielded_inputs`].
    pub async fn prepare_shielded_swap(
        self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> Result<PreparedTransfer, WalletError> {
        // Destination is our own seed so the build's `watch_for` tracks the
        // incoming coin.
        let payment = OutputInfo {
            destination: self.state.seed().into_ledger(),
            token_type: receive_token,
            value: receive_amount,
        };
        // No funding seed: an unbalanced half can't self-fund its Dust.
        self.prepare_shielded_offer(give_token, give_amount, Box::new(payment), false)
            .await
    }

    /// Spend this wallet's coins to cover `spend_amount` of `spend_token`, and
    /// put `payment` first in the offer, followed by the change (if any) back
    /// to this wallet.
    async fn prepare_shielded_offer(
        self,
        spend_token: ShieldedTokenType,
        spend_amount: u128,
        payment: Box<dyn helpers::BuildOutput<DefaultDB, BuildContext>>,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let seed = self.state.seed();
        let from_seed = seed.into_ledger();

        // Select the coins here rather than leaving `nullifier: None` for the
        // build to resolve: that path binds the single smallest coin covering
        // `spend_amount` and then substitutes the coin's own value for the
        // requested one, which caps a payment at the largest single coin and
        // drops whatever that coin holds beyond `spend_amount` with no output
        // to claim it.
        let (inputs, change) = InputInfo::coins_to_cover_value(
            self.context.clone(),
            from_seed.clone(),
            spend_amount,
            spend_token,
            self.coin_selection,
        )
        .map_err(|e| shielded_selection_error(&self.context, e))?;
        let spent_shielded_inputs = pinned_nullifiers(&inputs)?;

        let mut outputs = vec![payment];
        // Change back to this wallet, only if any.
        if change > 0 {
            outputs.push(Box::new(OutputInfo {
                destination: from_seed.clone(),
                token_type: spend_token,
                value: change,
            }));
        }

        // The wallet performs the spends, so the offer holds finished inputs
        // rather than a seed it would resolve during `build`.
        let context = self.context.clone();
        let mut tx_info =
            StandardTransactionInfo::new_from_context(self.context, self.proof_provider, None);
        let prepared = super::inputs::prepare_shielded_inputs(
            &context,
            seed,
            &spent_shielded_inputs,
            &mut tx_info.rng.split(),
        )?;

        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: prepared
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn helpers::BuildInput<DefaultDB, BuildContext>>)
                .collect(),
            outputs,
            transients: vec![],
        });
        // Fund the Dust fee from our own seed unless this is a Dustless build
        // (another wallet will sponsor the fees).
        if pay_fees {
            tx_info.set_funding_seeds(vec![from_seed]);
        }
        tx_info.use_mock_proofs_for_fees(true);

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.set_spent_shielded_inputs(spent_shielded_inputs);
        Ok(prepared)
    }

    /// Build an unshielded (UTXO) transfer transaction.
    ///
    /// `recipient` is a bech32 unshielded address (e.g.
    /// `mn_addr_undeployed1...`). Only the recipient's `user_address` (the
    /// public part) is needed; the chain derives the output's owner field
    /// directly from it. The change output, if any, goes back to the
    /// sender's own seed-derived address.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for another wallet to sponsor (the
    /// `.without_dust()` path); see [`Self::prepare_shielded`] for the multi-party flow.
    pub async fn prepare_unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let from_seed = self.state.seed().into_ledger();
        let recipient_wallet = parse_unshielded_recipient(recipient, self.state.network())?;

        let selected = UtxoSpendInfo::utxos_to_cover_value(
            self.context.clone(),
            from_seed.clone(),
            amount,
            token_type,
            self.coin_selection,
        )
        .await;
        let (spend_infos, change) = match selected {
            Ok(selected) => selected,
            Err(e) => return Err(utxo_selection_error(&self.context, e).await),
        };
        let spent_unshielded_inputs = spent_utxo_keys(&spend_infos);

        let mut outputs: Vec<Box<dyn helpers::BuildUtxoOutput<DefaultDB, BuildContext>>> =
            vec![Box::new(UtxoOutputInfo {
                value: amount,
                owner: recipient_wallet,
                token_type,
            })];

        if change > 0 {
            outputs.push(Box::new(UtxoOutputInfo {
                value: change,
                owner: from_seed.clone(),
                token_type,
            }));
        }

        let unshielded_offer = UnshieldedOfferInfo {
            inputs: spend_infos
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn helpers::BuildUtxoSpend<DefaultDB, BuildContext>>)
                .collect(),
            outputs,
        };

        let intent_info: IntentInfo<DefaultDB, BuildContext> = IntentInfo {
            guaranteed_unshielded_offer: Some(unshielded_offer),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        let mut tx_info =
            StandardTransactionInfo::new_from_context(self.context, self.proof_provider, None);
        tx_info.add_intent(1, Box::new(intent_info));
        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: vec![],
            outputs: vec![],
            transients: vec![],
        });
        // Fund the Dust fee from our own seed unless this is a Dustless build
        // (another wallet will sponsor the fees).
        if pay_fees {
            tx_info.set_funding_seeds(vec![from_seed]);
        }
        tx_info.use_mock_proofs_for_fees(true);

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.set_spent_unshielded_inputs(spent_unshielded_inputs);
        Ok(prepared)
    }

    /// Build a dust address registration transaction.
    ///
    /// Spends and re-creates one tNIGHT UTXO while registering the dust
    /// address. Uses "generationless fee availability" (virtual dust accrued
    /// by holding tNIGHT) to self-fund the registration fee, so a wallet needs
    /// no dust to register.
    ///
    /// One UTXO per call, because a second unshielded input in the same
    /// transaction costs more verification than its bytes buy in dismissal
    /// allowance, and the ledger refuses it with
    /// `FeeCalculation(OutsideTimeToDismiss)`, custom error 168.
    ///
    /// A registration covers the UTXO it spends, and it registers the address,
    /// so tNIGHT arriving afterwards generates dust with no further call. It
    /// does not reach back: tNIGHT the wallet already held stays outside dust
    /// generation until a registration spends it. Call this until it reports
    /// that every tNIGHT UTXO generates dust, which is
    /// [`crate::DustBalance::unregistered_night_utxos`] calls.
    ///
    /// The UTXO chosen is the one carrying the most generationless dust, which
    /// is value against age, so the fee has the most room.
    ///
    /// Skips tNIGHT UTXOs that already generate dust, and errors when every
    /// one of them does. The ledger grants no generationless availability for
    /// such a UTXO, so a registration that included one would declare a fee
    /// allowance the node rejects.
    ///
    /// `utxo_ctime` is a fallback creation timestamp (seconds since epoch),
    /// used only for UTXOs whose own creation time the indexer did not report.
    /// If `None`, those UTXOs fall back to `now - 1 hour`.
    pub async fn prepare_register_dust(
        self,
        utxo_ctime: Option<u64>,
    ) -> Result<PreparedTransfer, WalletError> {
        let seed = self.state.seed().into_ledger();

        let all_night: Vec<_> = self
            .state
            .unshielded_utxos()
            .iter()
            .filter(|u| u.is_night())
            .collect();

        if all_night.is_empty() {
            return Err(WalletError::Transfer(
                "no tNIGHT UTXOs available for dust registration".into(),
            ));
        }

        let dust_params = &self.state.parameters().dust;
        let now = self.context.latest_block_context().tblock;
        let fallback_ctime = match utxo_ctime {
            Some(t) => Timestamp::from_secs(t),
            None => Timestamp::from_secs(now.to_secs().saturating_sub(3600)),
        };
        let availability = |u: &TrackedUtxo| {
            let ctime = u
                .ctime
                .and_then(|t| u64::try_from(t).ok())
                .map_or(fallback_ctime, Timestamp::from_secs);
            generationless_fee_availability(
                u.value,
                ctime,
                dust_params.night_dust_ratio,
                dust_params.generation_decay_rate,
                now,
            )
        };

        // One input, the one carrying the most generationless dust. A second
        // unshielded input costs more verification than its bytes buy in
        // dismissal allowance, and the ledger refuses the transaction with
        // `FeeCalculation(OutsideTimeToDismiss)`, which reaches a client as
        // custom error 168.
        //
        // A UTXO whose creation time the indexer left out is ranked below
        // every UTXO that has one. Its age is a guess, and a guess that runs
        // ahead of the truth declares an allowance the ledger will not grant.
        let chosen = choose_registration_input(&all_night, availability).ok_or_else(|| {
            WalletError::Transfer(
                "every tNIGHT UTXO already generates dust: this address is registered".into(),
            )
        })?;
        let night_utxos = [chosen];

        let inputs: Vec<Box<dyn BuildUtxoSpend<DefaultDB, BuildContext>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoSpendInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                    intent_hash: Some(helpers::IntentHash(utxo.intent_hash)),
                    output_number: Some(utxo.output_index),
                };
                Box::new(info) as Box<dyn BuildUtxoSpend<DefaultDB, BuildContext>>
            })
            .collect();

        let outputs: Vec<Box<dyn BuildUtxoOutput<DefaultDB, BuildContext>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoOutputInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                };
                Box::new(info) as Box<dyn BuildUtxoOutput<DefaultDB, BuildContext>>
            })
            .collect();

        // The registered input belongs in the guaranteed offer. The ledger sums
        // the availability backing `allow_fee_payment` over the parent
        // intent's guaranteed inputs only, and creates the initial dust
        // outputs from that same offer, so a UTXO in the fallible leg is
        // declared but never counted and the node rejects the registration.
        let intent = IntentInfo {
            guaranteed_unshielded_offer: Some(UnshieldedOfferInfo { inputs, outputs }),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        // Declared over the same input the guaranteed offer carries. The
        // ledger sums availability over that offer's inputs, so a declaration
        // covering anything else is rejected.
        let allow_fee_payment = availability(chosen);

        let mut tx_info = StandardTransactionInfo::new_from_context(
            self.context.clone(),
            self.proof_provider.clone(),
            None,
        );
        tx_info.add_intent(Segment::Fallible.into(), Box::new(intent));
        tx_info.add_dust_registration(DustRegistrationBuilder {
            wallet: UnshieldedWallet::default(seed.clone()),
            dust_address: Some(DustWallet::<DefaultDB>::default(seed.clone(), None).public_key),
            allow_fee_payment,
        });
        tx_info.use_mock_proofs_for_fees(true);

        let spent_unshielded_inputs: Vec<SpentUtxoKey> =
            night_utxos.iter().map(|u| u.spent_key()).collect();

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.set_spent_unshielded_inputs(spent_unshielded_inputs);
        Ok(prepared)
    }
}

/// Pick the tNIGHT UTXO a registration should spend, or `None` when every one
/// of them already generates dust.
///
/// Ranked by the generationless dust the UTXO carries, which is its value
/// against its age, so the registration fee has the most room. A UTXO whose
/// creation time the indexer left out ranks below every UTXO that has one:
/// `availability` has to guess its age, and a guess that runs ahead of the
/// truth declares an allowance the ledger will not grant.
fn choose_registration_input<'a>(
    night_utxos: &[&'a TrackedUtxo],
    availability: impl Fn(&TrackedUtxo) -> u128,
) -> Option<&'a TrackedUtxo> {
    night_utxos
        .iter()
        .copied()
        .filter(|u| !u.is_registered_for_dust())
        .max_by_key(|u| (u.ctime.is_some(), availability(u)))
}

/// Mirror of the ledger's `generationless_fee_availability` for one UTXO. The
/// UTXO ages from its own creation time `ctime` to `now`, the
/// `DustActions.ctime` that the builder stamps.
fn generationless_fee_availability(
    value: u128,
    ctime: Timestamp,
    night_dust_ratio: u64,
    generation_decay_rate: u32,
    now: Timestamp,
) -> u128 {
    let dt = u128::try_from((now - ctime).as_seconds()).unwrap_or(0);
    let vfull = value.saturating_mul(night_dust_ratio as u128);
    let rate = value.saturating_mul(generation_decay_rate as u128);
    u128::min(dt.saturating_mul(rate), vfull)
}

/// The proven transaction plus the dust batches that funded it.
///
/// Each [`DustSpendBatch`] groups per-seed `(spends, updated_state)` from a
/// single `speculative_spend` call, which `mark_spent` takes together.
pub struct BuiltTransaction {
    pub finalized: FinalizedTx,
    pub dust_batches: Vec<DustSpendBatch>,
}

/// The chain time of `context`, and the TTL derived from it: that time plus the
/// ledger's `global_ttl`.
fn now_and_ttl(context: &BuildContext) -> (Timestamp, Timestamp) {
    let now = context.latest_block_context().tblock;
    let ttl = now + context.with_ledger_state(|ls| ls.parameters.global_ttl);
    (now, ttl)
}

/// Assemble the unproven transaction `tx_info` describes, with its offers and
/// intents built but no Dust attached and nothing proven. Also returns the
/// chain time it read and the TTL derived from it, which fee balancing needs.
async fn assemble_unproven(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<(UnprovenTx, Timestamp, Timestamp), WalletError> {
    let (now, ttl) = now_and_ttl(&tx_info.context);

    let guaranteed_offer = tx_info
        .guaranteed_offer
        .as_mut()
        .map(|gc| gc.build(&mut tx_info.rng, tx_info.context.clone()))
        .transpose()
        .map_err(transfer_err("build guaranteed offer"))?;

    let mut fallible_offers_vec = Vec::new();
    for (segment_id, offer_info) in tx_info.fallible_offers.iter_mut() {
        let offer = offer_info
            .build(&mut tx_info.rng, tx_info.context.clone())
            .map_err(transfer_err("build fallible offer"))?;
        fallible_offers_vec.push((*segment_id, offer));
    }
    let fallible_offer = fallible_offers_vec.into_iter().collect();

    let mut intents = HashMapStorage::<u16, helpers::UnprovenIntent<DefaultDB>, DefaultDB>::new();
    for (segment_id, intent_info) in tx_info.intents.iter_mut() {
        let intent = intent_info
            .build(&mut tx_info.rng, ttl, tx_info.context.clone(), *segment_id)
            .await;
        intents = intents.insert(*segment_id, intent);
    }

    let network_id = tx_info
        .context
        .with_ledger_state(|ls| ls.network_id.clone());

    let tx = Transaction::new(network_id, intents, guaranteed_offer, fallible_offer);

    Ok((tx, now, ttl))
}

/// Build and prove a transaction without the helpers' final `well_formed()`
/// check. The chain validates with its own `root_history`; matching that
/// locally would require a 55MB+ global `DustState`. Matches midnight-js.
pub async fn build_no_validate(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<BuiltTransaction, WalletError> {
    if pays_own_fee(&tx_info) && !tx_info.mock_proofs_for_fees {
        let (tx, now, ttl) = assemble_unproven(&mut tx_info).await?;
        return balance_fees_with_proofs(&mut tx_info, tx, now, ttl).await;
    }
    let PreparedTransfer {
        mut tx_info,
        tx,
        dust_batches,
        ..
    } = prepare_no_validate(tx_info).await?;
    let finalized = prove_tx_no_validate(&mut tx_info, tx).await?;
    Ok(BuiltTransaction {
        finalized,
        dust_batches,
    })
}

/// Whether `tx_info` draws Dust for its own fee: it names a funding seed or
/// registers a Dust address.
fn pays_own_fee(tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>) -> bool {
    !tx_info.funding_seeds.is_empty() || !tx_info.dust_registrations.is_empty()
}

/// Select the inputs and balance the fee, and stop before proving.
///
/// Everything here reads the build context rather than the wallet, so a caller
/// holding a wallet can prepare, record what the build will spend, release the
/// wallet, and prove afterwards. Proving needs no wallet and is far the slowest
/// step, so keeping it out of that span is what stops one build blocking every
/// other consumer.
///
/// Requires mock fee proofs when the build funds itself. Balancing without
/// them has to prove each round, which is the cost preparing exists to avoid.
pub async fn prepare_no_validate(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<PreparedTransfer, WalletError> {
    let (tx, now, ttl) = assemble_unproven(&mut tx_info).await?;

    let (tx, dust_batches) = if !pays_own_fee(&tx_info) {
        (tx, Vec::new())
    } else if tx_info.mock_proofs_for_fees {
        balance_fees_with_mocks(&mut tx_info, tx, now, ttl)?
    } else {
        return Err(WalletError::Transfer(
            "cannot prepare a self-funded build that prices its fee with real proofs; \
             it would prove on every balancing round"
                .into(),
        ));
    };

    Ok(PreparedTransfer::new(tx_info, tx, dust_batches))
}

/// The first segment [`fee_segment`] offers a fee intent.
///
/// A build puts its own intents at the low segments, and a later merge may
/// need those, so the fee starts high. Dust balances across segments, so
/// which one it lands on does not matter for the fee itself.
const DUST_FEE_SEGMENT: u16 = 0xFEED;

/// The first segment from [`DUST_FEE_SEGMENT`] up that `external` leaves
/// free. A merge refuses two intents at one segment, and a transaction that
/// was sponsored once already carries a fee intent at the first candidate.
fn fee_segment(external: &FinalizedTx) -> Result<u16, WalletError> {
    let Transaction::Standard(tx) = external else {
        return Err(WalletError::Transfer(
            "only a standard transaction can carry a fee intent".into(),
        ));
    };
    (DUST_FEE_SEGMENT..=u16::MAX)
        .find(|segment| tx.intents.get(segment).is_none())
        .ok_or_else(|| WalletError::Transfer("no intent segment left for the fee".into()))
}

/// Draw the fee for a finished transaction this wallet did not build, and
/// stop before proving.
///
/// The fee rides an intent of its own at a segment `external` does not use.
/// [`PreparedTransfer::prove`] merges it in after proving, so `external` and
/// its proofs stay as they are.
///
/// `None` when `external` already balances its Dust. There is nothing to
/// draw then, and an intent with no spends is one the ledger rejects.
pub async fn balance_external(
    tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    external: &FinalizedTx,
) -> Result<Option<PreparedTransfer>, WalletError> {
    let segment = fee_segment(external)?;
    balance_external_at(tx_info, external, segment).await
}

/// Balance the tokens of a finished transaction this wallet did not build
/// from `payer`'s coins, draw its fee, and stop before proving.
///
/// For each shielded token that a segment of `external` lacks, `payer`
/// spends coins into an offer at that segment and takes the change. For each
/// shielded token that a segment holds in surplus, `payer` takes the surplus
/// as an output. The offer for segment 0 is the guaranteed offer, and the
/// offer for any other segment is a fallible offer at that segment. An
/// unshielded token at segment 0 works the same way, in a guaranteed
/// unshielded offer on the fee intent. An unshielded surplus at a fallible
/// segment stays in `external`, because `payer` has no intent there to take
/// it in. Then the fee is drawn as [`balance_external`] draws it, and
/// [`PreparedTransfer::prove`] merges all of it into `external` after
/// proving.
///
/// The coins and UTXOs come from `payer`'s funding view in the context of
/// `tx_info`, in the default [`CoinSelectionStrategy`] order. `payer` must
/// be a funding seed of `tx_info`, so its Dust pays the fee too.
///
/// This pays every deficit that `external` carries. Call it only on a
/// transaction whose deficit `payer` agreed to cover.
///
/// `None` when `external` balances every token and its Dust.
///
/// # Errors
///
/// [`WalletError::FallibleUnshieldedDeficit`] when a fallible segment of
/// `external` lacks an unshielded token, before anything is selected.
/// [`WalletError::InsufficientShielded`] or
/// [`WalletError::InsufficientUnshielded`] when `payer` holds too little of a
/// token, and [`WalletError::InsufficientDust`] when it holds too little Dust
/// for the fee.
///
/// # Panics
///
/// Panics when the context of `tx_info` holds no funding view for `payer`.
pub async fn balance_external_tokens(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    payer: &Seed,
    external: &FinalizedTx,
) -> Result<Option<PreparedTransfer>, WalletError> {
    let segment = fee_segment(external)?;
    let imbalance = external.balance(None).map_err(transfer_err("balance"))?;
    let cover = token_cover(&imbalance)?;
    let shielded = add_shielded_cover(&mut tx_info, payer, cover.shielded)?;
    let unshielded = add_unshielded_cover(&mut tx_info, payer, cover.unshielded, segment).await?;
    let mut prepared = balance_external_at(tx_info, external, segment).await?;
    if let Some(prepared) = prepared.as_mut() {
        prepared.set_spent_shielded_inputs(shielded);
        prepared.set_spent_unshielded_inputs(unshielded);
    }
    Ok(prepared)
}

/// Draw the fee for `external` merged with what `tx_info` adds, with the fee
/// on the intent at `segment`, and stop before proving.
///
/// Each round prices the merge with a mock proof. That is exact here because
/// what `tx_info` adds carries only builtin proofs: Zswap spends and outputs,
/// and Dust spends.
///
/// `None` when `tx_info` adds nothing and `external` already balances its
/// Dust.
async fn balance_external_at(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    external: &FinalizedTx,
    segment: u16,
) -> Result<Option<PreparedTransfer>, WalletError> {
    let adds_tokens = tx_info.guaranteed_offer.is_some()
        || !tx_info.fallible_offers.is_empty()
        || !tx_info.intents.is_empty();
    let (tx, now, ttl) = assemble_unproven(&mut tx_info).await?;

    let mut tracker = FeeBalanceTracker::default();
    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let (candidate, batches) =
            dust_candidate(&tx_info, &tx, tracker.request(), now, ttl, segment)?;
        // The first round draws no Dust, so with no tokens to add it prices
        // `external` as it is.
        let adds = adds_tokens || batches.iter().any(|b| !b.spends.is_empty());
        let (fee, shortfall) = if adds {
            let mock = candidate.mock_prove().map_err(transfer_err("mock_prove"))?;
            let merged = external
                .merge(&mock)
                .map_err(transfer_err("merge fee intent"))?;
            compute_missing_dust(&tx_info, &merged)?
        } else {
            compute_missing_dust(&tx_info, external)?
        };
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        if !adds {
            return Ok(None);
        }
        confirm_dust_spends(&mut tx_info, &batches);
        let mut prepared = PreparedTransfer::new(tx_info, candidate, batches);
        prepared.external = Some(external.clone());
        return Ok(Some(prepared));
    }
    Err(tracker.into_error())
}

/// The token imbalance of a transaction that a payer balances, by segment.
///
/// A negative value is a deficit that the payer covers from its own coins. A
/// positive value is a surplus that the payer takes as an output.
#[derive(Debug, Default, PartialEq)]
struct TokenCover {
    /// The shielded imbalance, by the segment of the offer that balances it.
    shielded: BTreeMap<u16, Vec<(ShieldedTokenType, i128)>>,
    /// The unshielded imbalance at segment 0.
    unshielded: Vec<(UnshieldedTokenType, i128)>,
}

/// Sort the imbalance of a transaction into what a payer balances.
///
/// Dust is the fee, which the fee rounds pay, so it is left out. So is an
/// unshielded surplus at a fallible segment, which the payer has no intent
/// to take in.
///
/// # Errors
///
/// [`WalletError::FallibleUnshieldedDeficit`] for an unshielded deficit at a
/// fallible segment.
fn token_cover(imbalance: &BTreeMap<(TokenType, u16), i128>) -> Result<TokenCover, WalletError> {
    let guaranteed: u16 = Segment::Guaranteed.into();
    let mut cover = TokenCover::default();
    for (&(token_type, segment), &value) in imbalance {
        match token_type {
            _ if value == 0 => {}
            TokenType::Dust => {}
            TokenType::Shielded(token) => {
                cover
                    .shielded
                    .entry(segment)
                    .or_default()
                    .push((token, value));
            }
            TokenType::Unshielded(token) if segment == guaranteed => {
                cover.unshielded.push((token, value));
            }
            TokenType::Unshielded(token) if value < 0 => {
                return Err(WalletError::FallibleUnshieldedDeficit {
                    token_type: token.into_sdk(),
                    segment,
                    deficit: value.unsigned_abs(),
                });
            }
            TokenType::Unshielded(_) => {}
        }
    }
    Ok(cover)
}

/// Put `payer`'s side of each shielded imbalance in an offer at its segment,
/// and return the coins those offers spend.
fn add_shielded_cover(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    payer: &Seed,
    cover: BTreeMap<u16, Vec<(ShieldedTokenType, i128)>>,
) -> Result<Vec<Nullifier>, WalletError> {
    let seed = payer.into_ledger();
    let mut spent = Vec::new();
    for (segment, imbalance) in cover {
        let mut nullifiers = Vec::new();
        let mut outputs: Vec<Box<dyn helpers::BuildOutput<DefaultDB, BuildContext>>> = Vec::new();
        for (token_type, value) in imbalance {
            let to_payer = if value > 0 {
                value.unsigned_abs()
            } else {
                let (inputs, change) = InputInfo::coins_to_cover_value(
                    tx_info.context.clone(),
                    seed.clone(),
                    value.unsigned_abs(),
                    token_type,
                    CoinSelectionStrategy::default(),
                )
                .map_err(|e| shielded_selection_error(&tx_info.context, e))?;
                nullifiers.extend(pinned_nullifiers(&inputs)?);
                change
            };
            if to_payer > 0 {
                outputs.push(Box::new(OutputInfo {
                    destination: seed.clone(),
                    token_type,
                    value: to_payer,
                }));
            }
        }

        let inputs = super::inputs::prepare_shielded_inputs(
            &tx_info.context,
            payer,
            &nullifiers,
            &mut tx_info.rng.split(),
        )?;
        // The spends leave each coin in the funding view, so drop it there:
        // a later segment that lacks the same token must not pick it again.
        tx_info
            .context
            .with_wallet_from_seed(seed.clone(), |wallet| {
                for nullifier in &nullifiers {
                    wallet.shielded.state.coins = wallet.shielded.state.coins.remove(nullifier);
                }
            });

        // A fallible offer is built at segment 0 like the guaranteed one.
        // `Transaction::new` moves each fallible offer to its segment.
        let offer = OfferInfo {
            inputs: inputs
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn helpers::BuildInput<DefaultDB, BuildContext>>)
                .collect(),
            outputs,
            transients: vec![],
        };
        if segment == u16::from(Segment::Guaranteed) {
            tx_info.set_guaranteed_offer(offer);
        } else {
            tx_info.fallible_offers.insert(segment, offer);
        }
        spent.extend(nullifiers);
    }
    Ok(spent)
}

/// Put `payer`'s side of the unshielded imbalance at segment 0 in a
/// guaranteed unshielded offer on an intent at `segment`, and return the UTXOs
/// that offer spends.
async fn add_unshielded_cover(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    payer: &Seed,
    cover: Vec<(UnshieldedTokenType, i128)>,
    segment: u16,
) -> Result<Vec<SpentUtxoKey>, WalletError> {
    if cover.is_empty() {
        return Ok(Vec::new());
    }
    let seed = payer.into_ledger();
    let mut spent = Vec::new();
    let mut inputs: Vec<Box<dyn BuildUtxoSpend<DefaultDB, BuildContext>>> = Vec::new();
    let mut outputs: Vec<Box<dyn BuildUtxoOutput<DefaultDB, BuildContext>>> = Vec::new();
    for (token_type, value) in cover {
        let to_payer = if value > 0 {
            value.unsigned_abs()
        } else {
            let selected = UtxoSpendInfo::utxos_to_cover_value(
                tx_info.context.clone(),
                seed.clone(),
                value.unsigned_abs(),
                token_type,
                CoinSelectionStrategy::default(),
            )
            .await;
            let (spends, change) = match selected {
                Ok(selected) => selected,
                Err(e) => return Err(utxo_selection_error(&tx_info.context, e).await),
            };
            spent.extend(spent_utxo_keys(&spends));
            inputs.extend(
                spends
                    .into_iter()
                    .map(|s| Box::new(s) as Box<dyn BuildUtxoSpend<DefaultDB, BuildContext>>),
            );
            change
        };
        if to_payer > 0 {
            outputs.push(Box::new(UtxoOutputInfo {
                value: to_payer,
                owner: seed.clone(),
                token_type,
            }));
        }
    }
    tx_info.add_intent(
        segment,
        Box::new(IntentInfo {
            guaranteed_unshielded_offer: Some(UnshieldedOfferInfo { inputs, outputs }),
            fallible_unshielded_offer: None,
            actions: vec![],
        }),
    );
    Ok(spent)
}

/// Upper bound on the rounds of a fee-balancing loop. Each round requests the
/// candidate's full dust need, so in practice the loop converges in 2-3
/// rounds; hitting the cap means the fee keeps growing faster than the spends
/// added to pay it.
const MAX_FEE_BALANCE_ITERATIONS: usize = 10;

/// Convergence bookkeeping for a fee-balancing loop.
///
/// Each round rebuilds the candidate transaction from the original unpaid
/// `tx`, so dust spends never accumulate across rounds; the request passed
/// to `gather_dust_spends` must therefore cover the WHOLE need, not just
/// the latest gap. `record_shortfall` maintains that running total: the
/// dust provided in a round is always exactly the previous total
/// (`gather_dust_spends` errors unless the request is met in full), so
/// `total += current shortfall` makes the new total exactly the failed
/// candidate's full dust need. The loop is thus a fixpoint iteration on
/// the fee (request the last computed need, reprice the bigger tx,
/// repeat), which converges once adding spends stops growing the fee.
#[derive(Debug, Default)]
struct FeeBalanceTracker {
    /// Failed rounds so far.
    iterations: usize,
    /// Running total dust need; the request for the next round.
    missing_dust: u128,
    /// Fee (with margin) of the last unbalanced candidate, in specks.
    last_fee: u128,
}

impl FeeBalanceTracker {
    /// Dust to request from the funding wallets this round.
    fn request(&self) -> u128 {
        self.missing_dust
    }

    /// Record a round that came up short: `fee` is the candidate's
    /// computed fee with margin, `shortfall` the dust still missing.
    fn record_shortfall(&mut self, fee: u128, shortfall: u128) {
        self.iterations += 1;
        self.missing_dust = self.missing_dust.saturating_add(shortfall);
        self.last_fee = fee;
    }

    fn into_error(self) -> WalletError {
        WalletError::Transfer(format!(
            "could not balance TX after {} iterations: last candidate needed {} specks of dust in total (last computed fee {} specks)",
            self.iterations, self.missing_dust, self.last_fee
        ))
    }
}

/// Converge the Dust fee with mock proofs, and stop before the real one.
///
/// Each round prices the candidate with `mock_prove`, which is exact for a
/// build carrying only builtin proofs (zswap spend/output, dust spend) because
/// their sizes are the constants the fee is computed from. A contract call's
/// proof size varies per circuit, and `mock_prove` refuses rather than
/// guessing, so such a build fails here instead of being underfunded.
///
/// Rounds are side-effect-free apart from `confirm_dust_spends` on the one
/// that converges: each round rebuilds the candidate from `tx`, so a later
/// round cannot double-spend what an earlier one selected.
fn balance_fees_with_mocks(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<(UnprovenTx, Vec<DustSpendBatch>), WalletError> {
    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let (paid_tx, batches) = dust_candidate(
            tx_info,
            &tx,
            tracker.request(),
            now,
            ttl,
            Segment::Fallible.into(),
        )?;
        let mock = paid_tx.mock_prove().map_err(transfer_err("mock_prove"))?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &mock)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        confirm_dust_spends(tx_info, &batches);
        return Ok((paid_tx, batches));
    }
    Err(tracker.into_error())
}

/// Converge the Dust fee with a real proof of each candidate, and return the
/// proof of the candidate that balances.
async fn balance_fees_with_proofs(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<BuiltTransaction, WalletError> {
    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let (paid_tx, batches) = dust_candidate(
            tx_info,
            &tx,
            tracker.request(),
            now,
            ttl,
            Segment::Fallible.into(),
        )?;
        let proven = prove_tx_no_validate(tx_info, paid_tx).await?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &proven)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        confirm_dust_spends(tx_info, &batches);
        return Ok(BuiltTransaction {
            finalized: proven,
            dust_batches: batches,
        });
    }
    Err(tracker.into_error())
}

/// `tx` paid with Dust spends that cover `request` on the intent at
/// `segment`, and the batches those spends come from.
fn dust_candidate(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: &UnprovenTx,
    request: u128,
    now: Timestamp,
    ttl: Timestamp,
    segment: u16,
) -> Result<(UnprovenTx, Vec<DustSpendBatch>), WalletError> {
    let batches = gather_dust_spends(tx_info, request, now)?;
    let mut paid_tx = tx.clone();
    apply_dust(
        tx_info,
        &mut paid_tx,
        &flat_spends(&batches),
        tx_info.rng.clone().split(),
        ttl,
        now,
        segment,
    );
    Ok((paid_tx, batches))
}

fn gather_dust_spends(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    required_amount: u128,
    ctime: Timestamp,
) -> Result<Vec<DustSpendBatch>, WalletError> {
    let mut batches: Vec<DustSpendBatch> = Vec::new();
    let mut remaining = required_amount;
    let params = tx_info.context.with_ledger_state(|ls| ls.parameters.dust);
    let wallets = tx_info
        .context
        .wallets
        .lock()
        .expect("wallets lock poisoned");
    for seed in &tx_info.funding_seeds {
        if remaining == 0 {
            break;
        }
        // `get`, not `get_mut`: gathering must not mutate wallet state.
        // The fee-balancing retries rely on `speculative_spend` (`&self`)
        // leaving the wallet untouched until `confirm_dust_spends` applies the
        // chosen batch via `mark_spent`.
        let wallet = wallets
            .get(seed)
            .ok_or_else(|| WalletError::Transfer("unrecognized wallet seed".into()))?;
        let (new_spends, updated_state) = wallet
            .dust
            .speculative_spend(remaining, ctime, &params)
            .map_err(transfer_err("speculative_spend"))?;
        for spend in &new_spends {
            remaining = remaining.saturating_sub(spend.v_fee);
        }
        batches.push(DustSpendBatch {
            seed: sdk_seed(seed),
            spends: new_spends,
            updated_state,
        });
    }
    if remaining > 0 {
        // A remainder means `speculative_spend` drew on every unreserved Dust
        // UTXO, so the spends hold all that the funding wallets can give.
        Err(WalletError::InsufficientDust {
            required: required_amount,
            available: required_amount - remaining,
        })
    } else {
        Ok(batches)
    }
}

fn confirm_dust_spends(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    batches: &[DustSpendBatch],
) {
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .expect("wallets lock poisoned");
    for batch in batches {
        if let Some(wallet) = wallets.get_mut(&(&batch.seed).into_ledger()) {
            wallet
                .dust
                .mark_spent(&batch.spends, batch.updated_state.clone());
        }
    }
}

/// Price a candidate transaction. Returns the fee (with margin, in
/// specks) and, if the candidate doesn't balance, the dust shortfall.
fn compute_missing_dust(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: &FinalizedTx,
) -> Result<(u128, Option<u128>), WalletError> {
    let fees = tx_info
        .context
        .with_ledger_state(|s| tx.fees_with_margin(&s.parameters, 3))
        .map_err(transfer_err("fees_with_margin"))?;
    let imbalances = tx.balance(Some(fees)).map_err(transfer_err("balance"))?;
    let dust_imbalance = imbalances
        .get(&(TokenType::Dust, Segment::Guaranteed.into()))
        .copied()
        .unwrap_or_default();
    if dust_imbalance < 0 {
        Ok((fees, Some(dust_imbalance.unsigned_abs())))
    } else {
        Ok((fees, None))
    }
}

fn apply_dust(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    tx: &mut UnprovenTx,
    spends: &[helpers::DustSpend<ProofPreimageMarker, DefaultDB>],
    mut rng: StdRng,
    ttl: Timestamp,
    now: Timestamp,
    segment_id: u16,
) {
    let Transaction::Standard(stx) = tx else {
        return;
    };

    if spends.is_empty() && tx_info.dust_registrations.is_empty() {
        return;
    }

    let mut intent = match stx.intents.get(&segment_id) {
        Some(intent) => (*intent).clone(),
        None => Intent::empty(&mut rng, ttl),
    };

    // Ledger 9 signs the dust actions as part of the intent, so they go in
    // unsigned first and every signature below covers the assembled intent.
    let unsigned: Vec<DustRegistration<Signature, DefaultDB>> = tx_info
        .dust_registrations
        .iter()
        .map(DustRegistrationBuilder::build_unsigned)
        .collect();
    intent.dust_actions = Some(Sp::new(DustActions {
        spends: spends.to_vec().into(),
        registrations: unsigned.into(),
        ctime: now,
    }));
    let data_to_sign = intent
        .erase_proofs()
        .erase_signatures()
        .data_to_sign(segment_id);

    // The offers were signed when the intent was built, before it had dust
    // actions, so those signatures no longer cover it.
    let (guaranteed_keys, fallible_keys) = tx_info
        .intents
        .get(&segment_id)
        .map(|info| info.unshielded_signing_keys(tx_info.context.clone()))
        .unwrap_or_default();
    intent.guaranteed_unshielded_offer = resign_offer(
        intent.guaranteed_unshielded_offer.as_ref(),
        &guaranteed_keys,
        &data_to_sign,
        &mut rng,
    );
    intent.fallible_unshielded_offer = resign_offer(
        intent.fallible_unshielded_offer.as_ref(),
        &fallible_keys,
        &data_to_sign,
        &mut rng,
    );

    let registrations = tx_info
        .dust_registrations
        .iter()
        .map(|registration| {
            let mut signed = registration.build_unsigned();
            signed.signature = Some(Sp::new(registration.wallet.sign(&mut rng, &data_to_sign)));
            signed
        })
        .collect::<Vec<_>>()
        .into();
    intent.dust_actions = Some(Sp::new(DustActions {
        spends: spends.to_vec().into(),
        registrations,
        ctime: now,
    }));
    stx.intents = stx.intents.insert(segment_id, intent);

    // Re-compute the binding randomness
    *tx = Transaction::new(
        stx.network_id.clone(),
        stx.intents.clone(),
        stx.guaranteed_coins.as_ref().map(|c| (**c).clone()),
        stx.fallible_coins
            .iter()
            .map(|sp| (*sp.0, (*sp.1).clone()))
            .collect(),
    );
}

/// Sign `offer` again over `data_to_sign`, pairing each input with the key at
/// the same position.
fn resign_offer(
    offer: Option<&Sp<UnshieldedOffer<Signature, DefaultDB>, DefaultDB>>,
    signing_keys: &[TransactionSigningKey],
    data_to_sign: &[u8],
    rng: &mut StdRng,
) -> Option<Sp<UnshieldedOffer<Signature, DefaultDB>, DefaultDB>> {
    offer.map(|offer| {
        let signatures: Vec<Signature> = offer
            .inputs
            .iter()
            .zip(signing_keys)
            .map(|(_input, key)| key.sign(rng, data_to_sign))
            .collect();
        Sp::new(UnshieldedOffer {
            inputs: offer.inputs.clone(),
            outputs: offer.outputs.clone(),
            signatures: signatures.into(),
        })
    })
}

/// Payload length of a shielded address: a 32-byte coin public key followed by
/// the 32-byte serialized encryption public key.
const SHIELDED_ADDRESS_DATA_LEN: usize = 64;

fn parse_wallet_address(s: &str, network: &Network) -> Result<WalletAddress, WalletError> {
    let addr = WalletAddress::from_str(s)
        .map_err(|e| WalletError::InvalidAddress(format!("bech32 decode: {e}")))?;
    check_address_network(&addr, network)?;
    Ok(addr)
}

/// Reject an address minted for a different network.
///
/// The HRP carries the network as a third `_`-separated segment, which mainnet
/// omits entirely (`mn_shield-addr1…`, per upstream `network_suffix`). The
/// upstream `TryFrom<&WalletAddress>` impls check the `mn` prefix and the
/// credential segment but never the network, so without this an address for
/// another chain decodes cleanly and builds a transfer to a key nobody here
/// controls.
fn check_address_network(addr: &WalletAddress, expected: &Network) -> Result<(), WalletError> {
    let hrp = addr.human_readable_part();
    // The network is everything after the credential segment, not just the
    // next segment: upstream appends `_{network_id}` verbatim, and a custom
    // network name may itself contain underscores.
    let actual = hrp.splitn(3, '_').nth(2);
    let want = match expected {
        Network::Mainnet => None,
        other => Some(other.as_str()),
    };
    if actual != want {
        return Err(WalletError::AddressNetworkMismatch {
            expected: expected.as_str().to_string(),
            actual: actual.unwrap_or(Network::Mainnet.as_str()).to_string(),
        });
    }
    Ok(())
}

fn parse_unshielded_recipient(s: &str, network: &Network) -> Result<UnshieldedWallet, WalletError> {
    let addr = parse_wallet_address(s, network)?;
    UnshieldedWallet::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not an unshielded address: {e:?}")))
}

/// Decode a `mn_shield-addr_*` bech32 string into the shielded wallet whose
/// keys it carries.
///
/// `network` is the network that the address must belong to, usually the
/// network of the wallet that spends. The function returns
/// [`WalletError::AddressNetworkMismatch`] for an address of any other
/// network, and [`WalletError::InvalidAddress`] for a string that is not a
/// shielded address.
pub(crate) fn shielded_destination(
    s: &str,
    network: &Network,
) -> Result<ShieldedWallet<DefaultDB>, WalletError> {
    let addr = parse_wallet_address(s, network)?;
    // Upstream asserts this length rather than returning its `InvalidCoinKeyLen`
    // variant, so a truncated address would abort the process.
    let len = addr.data().len();
    if len != SHIELDED_ADDRESS_DATA_LEN {
        return Err(WalletError::InvalidAddress(format!(
            "shielded address payload is {len} bytes, expected {SHIELDED_ADDRESS_DATA_LEN}"
        )));
    }
    ShieldedWallet::<DefaultDB>::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not a shielded address: {e:?}")))
}

#[cfg(test)]
mod tests {
    /// A fee-only transaction at a segment.
    fn intent_at(segment: u16) -> FinalizedTx {
        use helpers::SeedableRng;
        let mut rng = StdRng::seed_from_u64(7);
        let intent: helpers::UnprovenIntent<DefaultDB> =
            Intent::empty(&mut rng, Timestamp::from_secs(1));
        let tx: UnprovenTx =
            Transaction::from_intents("undeployed", HashMapStorage::new().insert(segment, intent));
        tx.mock_prove().expect("mock prove")
    }

    /// A transaction sponsored once already carries a fee intent at the first
    /// candidate, and a merge refuses two intents at one segment, so the next
    /// fee must land beside it rather than on it.
    #[test]
    fn the_fee_takes_the_first_segment_the_transaction_leaves_free() {
        assert_eq!(
            fee_segment(&intent_at(DUST_FEE_SEGMENT)).unwrap(),
            DUST_FEE_SEGMENT + 1
        );
        assert_eq!(
            fee_segment(&intent_at(DUST_FEE_SEGMENT + 1)).unwrap(),
            DUST_FEE_SEGMENT
        );
    }

    fn shielded(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(helpers::HashOutput([byte; 32]))
    }

    fn unshielded(byte: u8) -> UnshieldedTokenType {
        UnshieldedTokenType(helpers::HashOutput([byte; 32]))
    }

    /// A fallible offer proves its spends for its own segment, so a deficit
    /// balanced in another segment leaves the transaction unbalanced. And a
    /// dropped surplus burns what the other party gave.
    #[test]
    fn each_shielded_imbalance_is_balanced_at_its_own_segment() {
        let imbalance = BTreeMap::from([
            ((TokenType::Shielded(shielded(1)), 0), -5),
            ((TokenType::Shielded(shielded(1)), 3), -4),
            ((TokenType::Shielded(shielded(2)), 3), 2),
            ((TokenType::Shielded(shielded(3)), 0), 0),
        ]);
        let cover = token_cover(&imbalance).unwrap();
        assert_eq!(
            cover.shielded,
            BTreeMap::from([
                (0, vec![(shielded(1), -5)]),
                (3, vec![(shielded(1), -4), (shielded(2), 2)]),
            ])
        );
        assert!(cover.unshielded.is_empty());
    }

    /// The payer's unshielded inputs ride its own intent, and a merge refuses
    /// a second intent at a segment the transaction uses. A cover that went
    /// ahead would fail at the merge, after it reserved the payer's coins.
    #[test]
    fn an_unshielded_deficit_at_a_fallible_segment_is_refused() {
        let imbalance = BTreeMap::from([((TokenType::Unshielded(unshielded(4)), 2), -7)]);
        let err = token_cover(&imbalance).unwrap_err();
        assert!(
            matches!(
                err,
                WalletError::FallibleUnshieldedDeficit { token_type, segment: 2, deficit: 7 }
                    if token_type == unshielded(4).into_sdk()
            ),
            "got {err:?}"
        );
    }

    /// A funding view of `seed` that holds a coin of `token` for each value.
    fn payer_context(seed: &Seed, token: ShieldedTokenType, values: &[u128]) -> Arc<BuildContext> {
        use helpers::SeedableRng;
        let context = Arc::new(BuildContext::new_from_wallet_seeds(
            "undeployed",
            &[seed.into_ledger()],
        ));
        let mut rng = StdRng::seed_from_u64(3);
        context.with_wallet_from_seed(seed.into_ledger(), |wallet| {
            for &value in values {
                let coin = helpers::CoinInfo::new(&mut rng, value, token);
                wallet.shielded.state = wallet
                    .shielded
                    .state
                    .insert_coin(wallet.shielded.secret_keys(), coin)
                    .expect("insert coin");
            }
        });
        context
    }

    /// One token short at two segments must draw two coins: a spend leaves
    /// its coin in the funding view, and the node refuses a coin spent twice.
    /// Each offer must also return its change and take its segment's surplus,
    /// or the chain burns that value with no error.
    #[test]
    fn each_segment_spends_its_own_coins_and_returns_the_rest() {
        let seed = Seed::from_bytes(&[1; 32]).unwrap();
        let mut tx_info = StandardTransactionInfo::new_from_context(
            payer_context(&seed, shielded(1), &[10, 6]),
            Arc::new(helpers::LocalProofServer::new()),
            Some([0; 32]),
        );
        let cover = BTreeMap::from([
            (0, vec![(shielded(1), -5)]),
            (3, vec![(shielded(1), -4), (shielded(2), 2)]),
        ]);

        let spent = add_shielded_cover(&mut tx_info, &seed, cover).unwrap();
        // With no intent to build, the assembly never waits.
        let (tx, _, _) = assemble_unproven(&mut tx_info)
            .now_or_never()
            .expect("an assembly with no intent completes at once")
            .unwrap();

        assert!(
            spent.len() == 2 && spent[0] != spent[1],
            "each segment must spend a coin of its own: {spent:?}"
        );
        assert_eq!(
            tx.balance(None).unwrap(),
            BTreeMap::from([
                ((TokenType::Shielded(shielded(1)), 0), 5),
                ((TokenType::Shielded(shielded(1)), 3), 4),
                ((TokenType::Shielded(shielded(2)), 3), -2),
            ])
        );
    }

    /// The payer's unshielded side must ride the intent at the fee segment,
    /// which `external` leaves free: any other segment can collide at the
    /// merge. It must also return its change and take the surplus, or the
    /// chain burns that value with no error.
    #[test]
    fn the_unshielded_side_rides_the_fee_intent_and_returns_the_rest() {
        use helpers::mn_ledger::structure::{Utxo, UtxoMeta};

        let seed = Seed::from_bytes(&[1; 32]).unwrap();
        let context = Arc::new(BuildContext::new_from_wallet_seeds(
            "undeployed",
            &[seed.into_ledger()],
        ));
        let utxo = Utxo {
            value: 10,
            owner: UnshieldedWallet::default(seed.into_ledger()).user_address,
            type_: unshielded(4),
            intent_hash: helpers::IntentHash(helpers::HashOutput([9; 32])),
            output_no: 0,
        };
        {
            let mut guard = context.ledger_state.lock().unwrap();
            let mut ledger_state = (**guard).clone();
            ledger_state.utxo = Sp::new(ledger_state.utxo.insert(
                utxo,
                UtxoMeta {
                    ctime: Timestamp::from_secs(0),
                },
            ));
            *guard = Sp::new(ledger_state);
        }
        let mut tx_info = StandardTransactionInfo::new_from_context(
            context,
            Arc::new(helpers::LocalProofServer::new()),
            Some([0; 32]),
        );
        let cover = vec![(unshielded(4), -7), (unshielded(5), 3)];

        // With no proof server or node to reach, neither step ever waits.
        let spent = add_unshielded_cover(&mut tx_info, &seed, cover, DUST_FEE_SEGMENT)
            .now_or_never()
            .expect("a selection from the funding view completes at once")
            .unwrap();
        let (tx, _, _) = assemble_unproven(&mut tx_info)
            .now_or_never()
            .expect("an assembly from the funding view completes at once")
            .unwrap();

        assert_eq!(
            spent,
            vec![SpentUtxoKey {
                intent_hash: helpers::HashOutput([9; 32]),
                output_index: 0,
            }]
        );
        let Transaction::Standard(stx) = &tx else {
            panic!("a standard transaction");
        };
        assert_eq!(
            stx.intents.keys().collect::<Vec<_>>(),
            vec![DUST_FEE_SEGMENT]
        );
        assert_eq!(
            tx.balance(None).unwrap(),
            BTreeMap::from([
                ((TokenType::Unshielded(unshielded(4)), 0), 7),
                ((TokenType::Unshielded(unshielded(5)), 0), -3),
            ])
        );
    }

    /// Dust is the fee the fee rounds pay, and an unshielded surplus at a
    /// fallible segment is out of the payer's reach. Neither is a reason to
    /// refuse the unshielded deficit at segment 0.
    #[test]
    fn the_fee_and_an_unreachable_surplus_stay_out_of_the_cover() {
        let imbalance = BTreeMap::from([
            ((TokenType::Dust, 0), -100),
            ((TokenType::Unshielded(unshielded(4)), 0), -7),
            ((TokenType::Unshielded(unshielded(5)), 2), 3),
        ]);
        assert_eq!(
            token_cover(&imbalance).unwrap(),
            TokenCover {
                shielded: BTreeMap::new(),
                unshielded: vec![(unshielded(4), -7)],
            }
        );
    }

    fn night_utxo(value: u128, ctime: Option<i64>, registered: bool) -> TrackedUtxo {
        TrackedUtxo {
            owner: "owner".into(),
            token_type: crate::NIGHT,
            value,
            intent_hash: crate::HashOutput([0; 32]),
            output_index: 0,
            ctime,
            registered_for_dust_generation: Some(registered),
        }
    }

    /// Availability stands in for value against age, which is what the real
    /// one computes.
    fn by_value(u: &TrackedUtxo) -> u128 {
        u.value
    }

    /// The None row also shows that a guessed creation time is still used
    /// when it is all there is. The rows put the UTXO with the most dust on
    /// opposite sides, so a pick of the first or the last UTXO fails one row.
    #[test]
    fn the_registration_takes_the_utxo_carrying_the_most_dust() {
        for (ctime, first, second) in [(Some(100), 1, 9), (None, 9, 1)] {
            let a = night_utxo(first, ctime, false);
            let b = night_utxo(second, ctime, false);
            assert_eq!(
                choose_registration_input(&[&a, &b], by_value).map(|u| u.value),
                Some(9),
                "ctime {ctime:?}"
            );
        }
    }

    #[test]
    fn a_registered_utxo_is_never_chosen() {
        let registered = night_utxo(9, Some(100), true);
        let free = night_utxo(1, Some(100), false);
        let all = [&registered, &free];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(1)
        );
    }

    #[test]
    fn every_utxo_registered_means_nothing_to_choose() {
        let a = night_utxo(9, Some(100), true);
        let all = [&a];
        assert!(choose_registration_input(&all, by_value).is_none());
    }

    /// A missing creation time forces the caller to guess an age, and a guess
    /// that runs ahead of the truth declares an allowance the ledger will not
    /// grant. So a guessed one loses to any real one, even a poorer one.
    #[test]
    fn a_utxo_without_a_creation_time_loses_to_one_that_has_it() {
        let guessed = night_utxo(9, None, false);
        let known = night_utxo(1, Some(100), false);
        let all = [&guessed, &known];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(1)
        );
    }

    use super::*;

    // Devnet and mainnet values, from the ledger's `INITIAL_DUST_PARAMETERS`.
    const RATIO: u64 = 5_000_000_000;
    const DECAY: u32 = 8267;

    fn avail(value: u128, ctime_secs: u64, now_secs: u64) -> u128 {
        generationless_fee_availability(
            value,
            Timestamp::from_secs(ctime_secs),
            RATIO,
            DECAY,
            Timestamp::from_secs(now_secs),
        )
    }

    /// Generation stops at the per-UTXO cap, so an ancient UTXO contributes
    /// `value * night_dust_ratio` and no more.
    #[test]
    fn generation_stops_at_the_cap() {
        let value = 1_000u128;
        let cap = value * RATIO as u128;
        // A UTXO reaches the cap after ceil(ratio / decay) seconds, about 7 days.
        let to_cap = (RATIO as u128).div_ceil(DECAY as u128) as u64;
        assert!(avail(value, 0, to_cap - 1) < cap);
        assert_eq!(avail(value, 0, to_cap), cap);
        assert_eq!(avail(value, 0, 10_000_000), cap);
    }

    /// A UTXO created at or after `now` has no age, so it adds nothing.
    #[test]
    fn a_utxo_with_no_age_adds_nothing() {
        assert_eq!(avail(1_000, 500, 500), 0);
        assert_eq!(avail(1_000, 900, 500), 0);
    }

    // The converging path of the fee-balancing loop (tracker is consulted
    // for the request, success returns without touching it) is exercised
    // end-to-end by the devnet tests in crates/midnight-wallet/tests/integration.rs
    // (`shielded_transfer_conserves_value` and others), which run the loop to
    // convergence.

    #[test]
    fn tracker_requests_the_accumulated_total() {
        let mut t = FeeBalanceTracker::default();
        // Round 1 gathers nothing (request 0), so the shortfall is the
        // whole fee of the unpaid candidate.
        assert_eq!(t.request(), 0);
        t.record_shortfall(500, 500);
        assert_eq!(t.request(), 500);
        // Round 2 provided 500 but the bigger tx costs 620: shortfall is
        // the *current* gap (120), and the next request must cover the
        // whole need (620) because each round rebuilds the candidate tx
        // from scratch.
        t.record_shortfall(620, 120);
        assert_eq!(t.request(), 620);
    }

    #[test]
    fn non_convergence_error_reports_shortfall_iterations_and_fee() {
        let mut t = FeeBalanceTracker::default();
        for i in 0..MAX_FEE_BALANCE_ITERATIONS as u128 {
            t.record_shortfall(1_000 + i, 7);
        }
        let WalletError::Transfer(msg) = t.into_error() else {
            panic!("expected WalletError::Transfer");
        };
        assert!(
            msg.contains("10 iterations"),
            "missing iteration count: {msg}"
        );
        assert!(
            msg.contains("70 specks"),
            "missing accumulated dust need: {msg}"
        );
        assert!(msg.contains("1009"), "missing last fee: {msg}");
    }
}
