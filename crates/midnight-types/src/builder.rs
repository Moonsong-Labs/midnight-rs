//! A transfer build: select the inputs, balance the Dust fee, and prove.
//!
//! A wallet implementation selects inputs and assembles a [`PreparedTransfer`]
//! with a [`TransferBuilder`]. Proving it ([`PreparedTransfer::prove`]) needs
//! the build context alone.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::FutureExt;
use midnight_helpers::{
    BlockContext, BuildContext, BuildIntent, BuilderContext, DefaultDB, DustActions,
    DustParameters, DustRegistration, DustSpend, DustWallet, FromContext, HashMapStorage, Intent,
    IntentHash, LedgerContext, LedgerParameters, LedgerState, MAX_SUPPLY, NIGHT, Nullifier, Offer,
    OfferInfo, OutputInfo, ProofPreimage, ProofPreimageMarker, ProofProvider, SecretKeys, Segment,
    ShieldedTokenType, ShieldedWallet, Signature, SignatureVerifyingKey, Sp, SplittableRng,
    StandardTransactionInfo, StdRng, Timestamp, TokenType, Transaction, UnprovenIntent,
    UnshieldedOffer, UnshieldedTokenType, UserAddress, UtxoOutput, UtxoSpend, WalletState,
};

use super::convert::{IntoLedger, IntoSdk};
use crate::error::refuse_repeated_nullifiers;
use crate::inputs::prepare_shielded_inputs;
use crate::request::{RequestKind, TransactionRequest};
use crate::sync::TrackedUtxo;
use crate::transfer::panic_message;
use crate::transfer::{ReservedInput, SpentInputs, SpentUtxoKey};
use crate::{DustPublicKey, Network, ShieldedRecipient, UnshieldedRecipient, WalletError};

type UnprovenTx = midnight_helpers::UnprovenTransaction<DefaultDB>;
type FinalizedTx = midnight_helpers::FinalizedTransaction<DefaultDB>;

type Spends = Vec<DustSpend<ProofPreimageMarker, DefaultDB>>;

/// What a build draws on from the wallet that funds it.
///
/// Each part leaves out what the pending builds of the wallet reserve, so a
/// build cannot select an input twice. It holds the keys that the proofs of
/// shielded and Dust spends need. It holds only the verifying key of the
/// unshielded UTXOs: a build writes a placeholder in each signature slot, and
/// the wallet signs after the proof.
pub struct Funding {
    /// The keys that spend the shielded coins.
    pub shielded_keys: SecretKeys,
    /// The shielded coins and their commitment tree.
    pub shielded: WalletState<DefaultDB>,
    /// The key that owns the unshielded UTXOs.
    pub night_key: SignatureVerifyingKey,
    /// The unshielded UTXOs.
    pub utxos: Vec<TrackedUtxo>,
    /// The Dust that pays the fee.
    pub dust: DustWallet<DefaultDB>,
}

impl Funding {
    /// The destination of a shielded output to this wallet.
    fn own_shielded(&self) -> ShieldedWallet<DefaultDB> {
        ShieldedWallet::from_pub_keys(
            self.shielded_keys.coin_public_key(),
            self.shielded_keys.enc_public_key(),
        )
    }

    /// The owner of an unshielded output to this wallet.
    fn own_address(&self) -> UserAddress {
        UserAddress::from(self.night_key.clone())
    }

    /// The spend of `utxo`, one of this wallet's UTXOs.
    fn spend(&self, utxo: &TrackedUtxo) -> UtxoSpend {
        UtxoSpend {
            value: utxo.value,
            owner: self.night_key.clone(),
            type_: utxo.token_type.into_ledger(),
            intent_hash: IntentHash(utxo.intent_hash),
            output_no: utxo.output_index,
        }
    }
}

/// An unshielded offer of `inputs` and `outputs`, in the order that the ledger
/// requires, with a placeholder signature for each input.
///
/// A placeholder has the size of a signature, so the fee that a build prices
/// counts the slot. The wallet replaces it after the proof, with
/// [`ProvenTx::add_signature`](crate::ProvenTx::add_signature).
fn placeholder_offer(
    mut inputs: Vec<UtxoSpend>,
    mut outputs: Vec<UtxoOutput>,
) -> UnshieldedOffer<Signature, DefaultDB> {
    inputs.sort();
    outputs.sort();
    UnshieldedOffer {
        signatures: vec![Signature::default(); inputs.len()].into(),
        inputs: inputs.into(),
        outputs: outputs.into(),
    }
}

/// An intent that carries one guaranteed unshielded offer.
///
/// The helpers' `IntentInfo` signs its offers with keys from the build
/// context. This intent signs nothing: its offer holds placeholders, which the
/// wallet replaces after the proof.
struct UnshieldedIntent(UnshieldedOffer<Signature, DefaultDB>);

#[async_trait]
impl BuildIntent<DefaultDB, BuildContext> for UnshieldedIntent {
    async fn build(
        &mut self,
        rng: &mut StdRng,
        ttl: Timestamp,
        _context: Arc<BuildContext>,
        _segment_id: u16,
    ) -> UnprovenIntent<DefaultDB> {
        let mut intent: UnprovenIntent<DefaultDB> = Intent::empty(rng, ttl);
        intent.guaranteed_unshielded_offer = Some(Sp::new(self.0.clone()));
        intent
    }
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
    dust_spends: Spends,
    spent_unshielded_inputs: Vec<SpentUtxoKey>,
    spent_shielded_inputs: Vec<Nullifier>,
    /// The [`chain_time`] of the build context that this build selected
    /// against. The reservation carries the same stamp, so a release can name
    /// this build's own entry.
    reserved_at: Timestamp,
    /// The time after which the transaction can no longer land.
    expires_at: Timestamp,
    /// The finished transaction that this build covers, which
    /// [`Self::prove`] merges the proven part into.
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
        dust_spends: Spends,
    ) -> Self {
        // Read both from the context and the transaction this build made
        // rather than take them as arguments, so they cannot disagree.
        let reserved_at = chain_time(&tx_info.context.latest_block_context());
        let expires_at = tx
            .intents()
            .map(|(_, intent)| intent.ttl)
            .min()
            .unwrap_or_else(|| longest_ttl(&tx_info.context));
        Self {
            tx_info,
            tx,
            dust_spends,
            spent_unshielded_inputs: Vec::new(),
            spent_shielded_inputs: Vec::new(),
            reserved_at,
            expires_at,
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
    /// The Dust spends that fund this build's fee.
    pub fn dust_spends(&self) -> &[DustSpend<ProofPreimageMarker, DefaultDB>] {
        &self.dust_spends
    }

    /// The unshielded UTXOs this build will spend.
    pub fn spent_unshielded_inputs(&self) -> &[SpentUtxoKey] {
        &self.spent_unshielded_inputs
    }

    /// The shielded coins this build will spend, by nullifier.
    pub fn spent_shielded_inputs(&self) -> &[Nullifier] {
        &self.spent_shielded_inputs
    }

    /// The [`chain_time`] of the build context that this build selected
    /// against. Reserve its inputs with this stamp, which
    /// [`Self::spent_inputs`] gives each of them.
    pub fn reserved_at(&self) -> Timestamp {
        self.reserved_at
    }

    /// The chain time after which this build's transaction can no longer
    /// land: the earliest TTL of its intents. Reserve its inputs until then.
    ///
    /// A transaction with no intent carries no TTL, and it gets the chain's
    /// `global_ttl` after the chain time of the build. A build from
    /// [`balance_external`] expires with the transaction that it covers, as
    /// that function describes.
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Everything this build will spend, in the form a reservation takes.
    pub fn spent_inputs(&self) -> SpentInputs {
        SpentInputs {
            dust: self.stamped(
                self.dust_spends
                    .iter()
                    .map(|spend| spend.old_nullifier.into_sdk()),
            ),
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

    /// Prove the transaction and seal it. The slowest step in a build, and it
    /// touches no wallet.
    ///
    /// A build from [`balance_external`] proves its part and merges it into
    /// the transaction that the part covers, and returns the merged
    /// transaction.
    pub async fn prove(self) -> Result<FinalizedTx, WalletError> {
        let PreparedTransfer {
            mut tx_info,
            tx,
            external,
            ..
        } = self;
        let proven = prove_tx_no_validate(&mut tx_info, tx).await?;
        match external {
            Some(external) => external
                .merge(&proven)
                .map_err(transfer_err("merge the part")),
            None => Ok(proven),
        }
    }
}

/// Wrap a foreign error as [`WalletError::Transfer`], naming the failing
/// step.
fn transfer_err<E: std::fmt::Debug>(ctx: &str) -> impl FnOnce(E) -> WalletError + '_ {
    move |e| WalletError::Transfer(format!("{ctx}: {e:?}"))
}

/// Take `candidates` of `(input, value)` largest first until their values
/// cover `required`. Returns the inputs and the change, or `None` when all of
/// them fall short.
///
/// Largest first spends the fewest inputs, and each shielded input carries its
/// own proof and adds to the fee. It never absorbs small coins. TODO: replace
/// it with best-fit selection,
/// <https://github.com/Moonsong-Labs/midnight-rs/issues/145>
fn select_largest_first<T>(
    mut candidates: Vec<(T, u128)>,
    required: u128,
) -> Option<(Vec<T>, u128)> {
    candidates.sort_by_key(|(_, value)| Reverse(*value));
    let mut total = 0u128;
    let mut selected = Vec::new();
    for (input, value) in candidates {
        total = total.checked_add(value)?;
        selected.push(input);
        if let Some(change) = total.checked_sub(required) {
            return Some((selected, change));
        }
    }
    None
}

/// The sum of `values`, which is what a funding view holds of one token.
fn held(values: impl IntoIterator<Item = u128>) -> u128 {
    values
        .into_iter()
        .fold(0u128, |sum, value| sum.saturating_add(value))
}

async fn prove_tx_no_validate(
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

/// A transfer that one wallet funds from its [`Funding`].
pub struct TransferBuilder {
    funding: Funding,
    context: Arc<BuildContext>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ttl: Option<std::time::Duration>,
}

impl TransferBuilder {
    pub fn new(
        funding: Funding,
        context: Arc<BuildContext>,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            funding,
            context,
            proof_provider,
            ttl: None,
        }
    }

    /// Prepare the transfer that `request` names, with the TTL and the fee
    /// that it asks for. Dispatches to the matching `prepare_*` method below,
    /// each of which documents what it builds.
    ///
    /// # Errors
    ///
    /// [`WalletError::Transfer`] for a sponsor or a contract part, which
    /// [`crate::build::fill`] fills without a transfer build.
    pub async fn prepare(
        self,
        request: TransactionRequest,
    ) -> Result<PreparedTransfer, WalletError> {
        let TransactionRequest {
            kind,
            pays_fee,
            ttl,
        } = request;
        let builder = Self { ttl, ..self };
        match kind {
            RequestKind::Shielded {
                token_type,
                amount,
                recipient,
            } => {
                builder
                    .prepare_shielded(token_type.into_ledger(), amount, recipient, pays_fee)
                    .await
            }
            RequestKind::Unshielded {
                token_type,
                amount,
                recipient,
            } => {
                builder
                    .prepare_unshielded(token_type.into_ledger(), amount, recipient, pays_fee)
                    .await
            }
            RequestKind::ShieldedSwap {
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
            RequestKind::RegisterDust { target } => builder.prepare_register_dust(target).await,
            RequestKind::Sponsor(_) | RequestKind::Contract(_) => Err(WalletError::Transfer(
                "a sponsor or a contract part is not a transfer".into(),
            )),
        }
    }

    /// Build a shielded (ZSwap) transfer transaction.
    ///
    /// `recipient` holds the recipient's `coin_public_key` and
    /// `enc_public_key`, which is all the chain needs to construct the output
    /// coin commitment and encrypt the coin info for them.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for a multi-party flow where another
    /// wallet pays the fees (`TransactionRequest::without_fee`). It is not
    /// submittable on its own; hand it to the fee payer, who completes it with
    /// `TransactionRequest::sponsor` and sends it.
    ///
    /// `amount` may span several coins: inputs are selected up front, largest
    /// first, and the remainder returns to this wallet as a change output, so a payment is bounded by the wallet's total
    /// balance in `token_type` rather than by its largest single coin. Sending
    /// the full balance to this wallet's own address therefore consolidates it
    /// into one coin, and sending part of a coin splits it. The spent coins'
    /// nullifiers are in [`PreparedTransfer::spent_shielded_inputs`].
    pub async fn prepare_shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: ShieldedRecipient,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let payment = OutputInfo {
            destination: ShieldedWallet::<DefaultDB>::from_pub_keys(
                recipient.coin_public_key.into_ledger(),
                recipient.enc_public_key.into_ledger(),
            ),
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
    /// a sponsor funds (`TransactionRequest::sponsor`) and sends.
    ///
    /// Because the half is unbalanced it can't self-fund its Dust, so it is
    /// always fee-less: the build draws no Dust. Give-side coins are selected
    /// up front, largest first; the resulting change (if any) goes back to
    /// this wallet, mirroring [`Self::prepare_unshielded`]'s change handling in
    /// the shielded domain. The spent coins' nullifiers are in
    /// [`PreparedTransfer::spent_shielded_inputs`].
    pub async fn prepare_shielded_swap(
        self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> Result<PreparedTransfer, WalletError> {
        let payment = OutputInfo {
            destination: self.funding.own_shielded(),
            token_type: receive_token,
            value: receive_amount,
        };
        // An unbalanced half can't self-fund its Dust.
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
        payment: Box<dyn midnight_helpers::BuildOutput<DefaultDB, BuildContext>>,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let TransferBuilder {
            mut funding,
            context,
            proof_provider,
            ttl,
        } = self;

        let coins: Vec<(Nullifier, u128)> = funding
            .shielded
            .coins
            .iter()
            .filter(|(_, coin)| coin.type_ == spend_token)
            .map(|(nullifier, coin)| (nullifier, coin.value))
            .collect();
        let available = held(coins.iter().map(|(_, value)| *value));
        let (spent_shielded_inputs, change) = select_largest_first(coins, spend_amount)
            .ok_or_else(|| WalletError::InsufficientShielded {
                token_type: spend_token.into_sdk(),
                required: spend_amount,
                available,
            })?;

        let mut outputs = vec![payment];
        // Change back to this wallet, only if any.
        if change > 0 {
            outputs.push(Box::new(OutputInfo {
                destination: funding.own_shielded(),
                token_type: spend_token,
                value: change,
            }));
        }

        // The wallet performs the spends, so the offer holds finished inputs
        // rather than a seed it would resolve during `build`.
        let mut tx_info = StandardTransactionInfo::new_from_context(context, proof_provider, None);
        let prepared = prepare_shielded_inputs(
            &mut funding.shielded,
            &funding.shielded_keys,
            &spent_shielded_inputs,
            &mut tx_info.rng.split(),
        )?;

        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: prepared
                .into_iter()
                .map(|i| {
                    Box::new(i) as Box<dyn midnight_helpers::BuildInput<DefaultDB, BuildContext>>
                })
                .collect(),
            outputs,
            transients: vec![],
        });
        tx_info.use_mock_proofs_for_fees(true);

        let fee = if pay_fees {
            Fee::Dust(&funding.dust)
        } else {
            Fee::None
        };
        let mut prepared = prepare_with_ttl(tx_info, fee, ttl).await?;
        prepared.set_spent_shielded_inputs(spent_shielded_inputs);
        Ok(prepared)
    }

    /// Build an unshielded (UTXO) transfer transaction.
    ///
    /// `recipient` is the user address that owns the output. The change
    /// output, if any, goes back to this wallet's own address.
    ///
    /// Each input holds a placeholder signature, which the wallet replaces
    /// after the proof.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for another wallet to sponsor
    /// (`TransactionRequest::without_fee`); see [`Self::prepare_shielded`] for
    /// the multi-party flow.
    pub async fn prepare_unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: UnshieldedRecipient,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let funding = &self.funding;
        let utxos: Vec<(&TrackedUtxo, u128)> = funding
            .utxos
            .iter()
            .filter(|utxo| utxo.token_type.into_ledger() == token_type)
            .map(|utxo| (utxo, utxo.value))
            .collect();
        let available = held(utxos.iter().map(|(_, value)| *value));
        let (selected, change) = select_largest_first(utxos, amount).ok_or_else(|| {
            WalletError::InsufficientUnshielded {
                token_type: token_type.into_sdk(),
                required: amount,
                available,
            }
        })?;

        let mut outputs = vec![UtxoOutput {
            value: amount,
            owner: recipient.into_ledger(),
            type_: token_type,
        }];
        if change > 0 {
            outputs.push(UtxoOutput {
                value: change,
                owner: funding.own_address(),
                type_: token_type,
            });
        }
        let offer = placeholder_offer(
            selected.iter().map(|utxo| funding.spend(utxo)).collect(),
            outputs,
        );
        let spent_unshielded_inputs: Vec<SpentUtxoKey> =
            selected.iter().map(|utxo| utxo.spent_key()).collect();

        let mut tx_info = StandardTransactionInfo::new_from_context(
            self.context.clone(),
            self.proof_provider.clone(),
            None,
        );
        tx_info.add_intent(1, Box::new(UnshieldedIntent(offer)));
        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: vec![],
            outputs: vec![],
            transients: vec![],
        });
        tx_info.use_mock_proofs_for_fees(true);

        let fee = if pay_fees {
            Fee::Dust(&funding.dust)
        } else {
            Fee::None
        };
        let mut prepared = prepare_with_ttl(tx_info, fee, self.ttl).await?;
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
    /// `FeeCalculation(OutsideTimeToDismiss)`, custom error 231.
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
    /// A UTXO whose own creation time the indexer did not report counts as
    /// created one hour before the chain time of the build.
    ///
    /// `target` is the Dust public key that gets the Dust of the registered
    /// tNIGHT, for example the key of another wallet that pays fees. `None`
    /// names this wallet's own key. The registration sets the Dust public key
    /// of the whole NIGHT address. A later registration replaces it, also when
    /// its `target` is `None`. tNIGHT that reaches the address after a
    /// registration generates Dust for the key that the last registration set.
    /// tNIGHT that already generates Dust keeps its key until the wallet spends
    /// it.
    ///
    /// The input and the registration each hold a placeholder signature,
    /// which the wallet replaces after the proof.
    pub async fn prepare_register_dust(
        self,
        target: Option<DustPublicKey>,
    ) -> Result<PreparedTransfer, WalletError> {
        let funding = &self.funding;
        let all_night: Vec<_> = funding.utxos.iter().filter(|u| u.is_night()).collect();

        if all_night.is_empty() {
            return Err(WalletError::Transfer(
                "no tNIGHT UTXOs available for dust registration".into(),
            ));
        }

        let dust_params = self.context.with_ledger_state(|ls| ls.parameters.dust);
        let now = self.context.latest_block_context().tblock;
        let fallback_ctime = Timestamp::from_secs(now.to_secs().saturating_sub(3600));
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
        // custom error 231.
        //
        // A UTXO whose creation time the indexer left out is ranked below
        // every UTXO that has one. Its age is a guess, and a guess that runs
        // ahead of the truth declares an allowance the ledger will not grant.
        let chosen = choose_registration_input(&all_night, availability).ok_or_else(|| {
            WalletError::Transfer(
                "every tNIGHT UTXO already generates dust: this address is registered".into(),
            )
        })?;

        // The registered input belongs in the guaranteed offer. The ledger sums
        // the availability backing `allow_fee_payment` over the parent
        // intent's guaranteed inputs only, and creates the initial dust
        // outputs from that same offer, so a UTXO in the fallible leg is
        // declared but never counted and the node rejects the registration.
        let offer = placeholder_offer(
            vec![funding.spend(chosen)],
            vec![UtxoOutput {
                value: chosen.value,
                owner: funding.own_address(),
                type_: NIGHT,
            }],
        );

        // Declared over the same input the guaranteed offer carries. The
        // ledger sums availability over that offer's inputs, so a declaration
        // covering anything else is rejected.
        let allow_fee_payment = availability(chosen);
        let dust_address = match target {
            Some(target) => target.into_ledger(),
            None => funding.dust.public_key,
        };
        // The seal keeps whether a registration holds a signature, so the
        // placeholder goes in before the proof.
        let registration = DustRegistration {
            night_key: funding.night_key.clone(),
            dust_address: Some(Sp::new(dust_address)),
            allow_fee_payment,
            signature: Some(Sp::new(Signature::default())),
        };

        let mut tx_info = StandardTransactionInfo::new_from_context(
            self.context.clone(),
            self.proof_provider.clone(),
            None,
        );
        tx_info.add_intent(Segment::Fallible.into(), Box::new(UnshieldedIntent(offer)));
        tx_info.use_mock_proofs_for_fees(true);

        let spent_unshielded_inputs = vec![chosen.spent_key()];
        let mut prepared =
            prepare_with_ttl(tx_info, Fee::Registration(Box::new(registration)), self.ttl).await?;
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

/// The latest chain time that `block_context` knows: its block time, or the
/// time of its last block when that is later.
///
/// A wallet anchors the block time of its build context at its Dust state,
/// which can trail the chain, and keeps the time of the chain's latest block
/// as the last block's time. A context that the helpers make for a block, or
/// with no block, holds an earlier last block time.
///
/// The ledger defines `last_block_time` as the time of the block before
/// `tblock`, so it is never after `tblock` on chain. The wallet's use is a
/// convention of this SDK: a transaction applied locally against a wallet's
/// context sees a `last_block_time` after `tblock`.
pub fn chain_time(block_context: &BlockContext) -> Timestamp {
    block_context.tblock.max(block_context.last_block_time)
}

/// The TTL a build gives its intents: `ttl` after the chain time of `context`
/// (see [`chain_time`]), or the chain's `global_ttl` after it when `ttl` is
/// `None`.
///
/// The chain counts a TTL in whole seconds, so this drops a fraction of a
/// second.
///
/// # Errors
///
/// [`WalletError::TtlOutOfRange`] when `ttl` is under one second, or longer
/// than the chain's `global_ttl`.
fn intent_ttl(
    context: &BuildContext,
    ttl: Option<std::time::Duration>,
) -> Result<Timestamp, WalletError> {
    let global_ttl = context.with_ledger_state(|ls| ls.parameters.global_ttl);
    ttl_after(chain_time(&context.latest_block_context()), global_ttl, ttl)
}

/// The longest TTL that the chain accepts for an intent built on `context`:
/// the chain's `global_ttl` after the chain time of `context` (see
/// [`chain_time`]).
fn longest_ttl(context: &BuildContext) -> Timestamp {
    chain_time(&context.latest_block_context())
        + context.with_ledger_state(|ls| ls.parameters.global_ttl)
}

/// The context a build executes against: a ledger of `network` with
/// `parameters`, the default proving resolver, and `block_context`. It holds
/// no wallet, so a build funds nothing from it until a wallet adds its view.
///
/// # Errors
///
/// [`WalletError::Transfer`] when the ledger refuses the genesis settings.
pub fn execution_context(
    network: impl Into<Network>,
    parameters: LedgerParameters,
    block_context: Option<BlockContext>,
) -> Result<Arc<BuildContext>, WalletError> {
    let network = network.into();
    // reserve_pool must equal MAX_SUPPLY to satisfy the NIGHT balance invariant.
    let ledger_state =
        LedgerState::with_genesis_settings(network.as_str(), parameters, 0, MAX_SUPPLY, 0)
            .map_err(transfer_err("construct ledger state"))?;
    Ok(Arc::new(LedgerContext {
        ledger_state: std::sync::Mutex::new(Sp::new(ledger_state)),
        latest_block_context: std::sync::Mutex::new(block_context),
        ..LedgerContext::new(network.as_str())
    }))
}

/// `ttl` after `chain_time`, or `global_ttl` after it for `None`. See
/// [`intent_ttl`].
fn ttl_after(
    chain_time: Timestamp,
    global_ttl: midnight_helpers::Duration,
    ttl: Option<std::time::Duration>,
) -> Result<Timestamp, WalletError> {
    let Some(requested) = ttl else {
        return Ok(chain_time + global_ttl);
    };
    let secs = i128::from(requested.as_secs());
    if secs == 0 || secs > global_ttl.as_seconds() {
        return Err(WalletError::TtlOutOfRange {
            requested,
            global_ttl: std::time::Duration::from_secs(
                u64::try_from(global_ttl.as_seconds()).unwrap_or_default(),
            ),
        });
    }
    Ok(chain_time + midnight_helpers::Duration::from_secs(secs))
}

/// Refuse `ttl` when the transaction that `tx_info` describes will have no
/// intent to carry it: it has none of its own, and it pays no fee
/// (`pays_fee`), which would add one.
fn refuse_ttl_without_intent(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    pays_fee: bool,
    ttl: Option<std::time::Duration>,
) -> Result<(), WalletError> {
    if ttl.is_some() && tx_info.intents.is_empty() && !pays_fee {
        return Err(WalletError::TtlWithoutIntent);
    }
    Ok(())
}

/// Assemble the unproven transaction `tx_info` describes, with its offers and
/// intents built but no Dust attached and nothing proven. Also returns the
/// block time it read and the TTL of its intents, which fee balancing needs.
async fn assemble_unproven(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    pays_fee: bool,
    ttl: Option<std::time::Duration>,
) -> Result<(UnprovenTx, Timestamp, Timestamp), WalletError> {
    refuse_ttl_without_intent(tx_info, pays_fee, ttl)?;
    let now = tx_info.context.latest_block_context().tblock;
    let ttl = intent_ttl(&tx_info.context, ttl)?;

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

    let mut intents =
        HashMapStorage::<u16, midnight_helpers::UnprovenIntent<DefaultDB>, DefaultDB>::new();
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

/// What pays the Dust fee of a build.
enum Fee<'a> {
    /// Another party pays it.
    None,
    /// The Dust of the wallet that funds the build pays it.
    Dust(&'a DustWallet<DefaultDB>),
    /// The generationless Dust of the tNIGHT that this registration spends
    /// pays it. The registration holds a placeholder signature.
    Registration(Box<DustRegistration<Signature, DefaultDB>>),
}

impl Fee<'_> {
    fn pays(&self) -> bool {
        !matches!(self, Fee::None)
    }

    fn dust(&self) -> Option<&DustWallet<DefaultDB>> {
        match self {
            Fee::Dust(dust) => Some(dust),
            Fee::None | Fee::Registration(_) => None,
        }
    }

    fn registration(&self) -> Option<&DustRegistration<Signature, DefaultDB>> {
        match self {
            Fee::Registration(registration) => Some(registration.as_ref()),
            Fee::None | Fee::Dust(_) => None,
        }
    }
}

/// Select the inputs and balance the fee, and stop before proving.
///
/// Everything here reads the build context rather than the wallet, so a caller
/// holding a wallet can prepare, record what the build will spend, release the
/// wallet, and prove afterwards. Proving needs no wallet and is far the slowest
/// step, so keeping it out of that span is what stops one build blocking every
/// other consumer.
///
/// `dust` pays the fee, from the [`Funding`] of the wallet. With `None`,
/// another party pays it.
///
/// Requires mock fee proofs when the build funds itself. Balancing without
/// them has to prove each round, which is the cost preparing exists to avoid.
///
/// The build skips the helpers' final `well_formed()` check, as midnight-js
/// does. The chain validates the transaction against its own `root_history`.
/// A local check can match that only with the global `DustState`, which is
/// 55MB or more.
pub async fn prepare_no_validate(
    tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    dust: Option<&DustWallet<DefaultDB>>,
) -> Result<PreparedTransfer, WalletError> {
    prepare_with_ttl(tx_info, dust.map_or(Fee::None, Fee::Dust), None).await
}

/// [`prepare_no_validate`], with the fee that `fee` pays and the intent TTL
/// that [`intent_ttl`] gives for `ttl`. It refuses a bad `ttl` before it
/// spends or proves anything.
async fn prepare_with_ttl(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    fee: Fee<'_>,
    ttl: Option<std::time::Duration>,
) -> Result<PreparedTransfer, WalletError> {
    let (tx, now, ttl) = assemble_unproven(&mut tx_info, fee.pays(), ttl).await?;

    let (tx, dust_spends) = if !fee.pays() {
        (tx, Vec::new())
    } else if tx_info.mock_proofs_for_fees {
        balance_fees_with_mocks(&tx_info, &fee, tx, now, ttl)?
    } else {
        return Err(WalletError::Transfer(
            "cannot prepare a self-funded build that prices its fee with real proofs; \
             it would prove on every balancing round"
                .into(),
        ));
    };

    Ok(PreparedTransfer::new(tx_info, tx, dust_spends))
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

/// The TTL of the cover's fee intent for `external`, and the expiry of
/// everything that the cover reserves. It is the earliest TTL of the intents
/// of `external`, kept between the chain time of `context` and
/// [`longest_ttl`]. With no intent, it is [`longest_ttl`].
///
/// The upper bound: `external` comes from another party, and an uncapped TTL
/// would let that party set how long the inputs of the wallet stay reserved.
///
/// The lower bound: a TTL that already passed would let the next build at the
/// same chain time evict this reservation. That build would take the same
/// inputs with the same stamp. A release of this build would then drop the
/// reservation of that build too. The wallet keeps a reservation while its TTL
/// is at or after the chain time. So a TTL at the chain time keeps the
/// reservation through every build at that chain time.
fn fee_ttl(external: &FinalizedTx, context: &BuildContext) -> Timestamp {
    let longest = longest_ttl(context);
    let Some(earliest) = external.intents().map(|(_, intent)| intent.ttl).min() else {
        return longest;
    };
    earliest
        .max(chain_time(&context.latest_block_context()))
        .min(longest)
}

/// What a wallet adds to a proven transaction that it did not build.
///
/// The wallet adds it in a part of its own, which [`PreparedTransfer::prove`]
/// proves and merges in, so the transaction and its proofs stay as they are.
#[derive(Debug, Clone)]
pub enum Cover {
    /// The Dust fee alone, as a sponsor pays it. A deficit of any other token
    /// is refused.
    Fee,
    /// Each shielded deficit, with the change back to the wallet, and the Dust
    /// fee when `pay_fees` is true.
    ///
    /// The part spends the coins of `pins` first. It then selects coins of the
    /// wallet for each deficit that the pins leave.
    Shortfalls {
        /// Coins of the wallet that the part spends whole, by nullifier.
        pins: Vec<crate::Nullifier>,
        /// Whether the part also pays the Dust fee.
        pay_fees: bool,
    },
}

/// Cover what `cover` names in `external`, a finished transaction that this
/// wallet did not build, and stop before proving.
///
/// The cover rides a part of its own. Its shielded inputs and change ride an
/// offer at the segment of each deficit that they cover, and its Dust rides an
/// intent at a segment `external` does not use. [`PreparedTransfer::prove`]
/// merges the part in after proving, so `external` and its proofs stay as they
/// are. Each round prices the merge with a mock proof, which is exact here
/// because the part carries only built-in proofs.
///
/// The shielded inputs and the Dust come from `funding`. The reservation of the part, and its fee intent, take the
/// earliest TTL of the intents of `external`, kept between the chain time of
/// the build (see [`chain_time`]) and the chain's `global_ttl` after it. So
/// what the part reserves expires when the merged transaction can no longer
/// land. When `external` has no intent, they get `global_ttl` after the chain
/// time.
///
/// `None` when the part has nothing to add: no shielded deficit to cover, no
/// pin, and no Dust to draw. An intent with no spends is one the ledger
/// rejects.
///
/// # Errors
///
/// [`WalletError::Transfer`] for a deficit that `cover` does not take,
/// [`WalletError::DuplicateInput`] when `cover` pins a coin twice,
/// [`WalletError::InsufficientShielded`] when the coins of the wallet do not
/// cover a deficit, and [`WalletError::InsufficientDust`] when its Dust does
/// not cover the fee.
pub fn balance_external(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    mut funding: Funding,
    external: &FinalizedTx,
    cover: &Cover,
) -> Result<Option<PreparedTransfer>, WalletError> {
    let now = tx_info.context.latest_block_context().tblock;
    let ttl = fee_ttl(external, &tx_info.context);
    let network_id = tx_info
        .context
        .with_ledger_state(|ls| ls.network_id.clone());
    let segment = fee_segment(external)?;

    let balance = external.balance(None).map_err(transfer_err("balance"))?;
    let deficits = deficits_to_cover(&balance, cover)?;
    let (pins, pay_fees) = match cover {
        Cover::Fee => (Vec::new(), true),
        Cover::Shortfalls { pins, pay_fees } => {
            (pins.iter().map(|n| n.into_ledger()).collect(), *pay_fees)
        }
    };
    let plan = plan_cover(&deficits, &pins, &free_coins(&funding.shielded))?;
    let offers = build_cover_offers(&mut tx_info, &mut funding, &plan)?;

    let mut tracker = FeeBalanceTracker::default();
    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        // The first round draws nothing and prices `external` with the offers.
        let spends = if pay_fees {
            gather_dust_spends(
                &tx_info.context,
                Some(&funding.dust),
                tracker.request(),
                now,
            )?
        } else {
            Vec::new()
        };
        let mut intents = HashMapStorage::new();
        if !spends.is_empty() {
            let mut intent: midnight_helpers::UnprovenIntent<DefaultDB> =
                Intent::empty(&mut tx_info.rng, ttl);
            intent.dust_actions = Some(Sp::new(DustActions {
                spends: spends.clone().into(),
                registrations: Vec::new().into(),
                ctime: now,
            }));
            intents = intents.insert(segment, intent);
        }
        let part = (!offers.is_empty() || !intents.is_empty())
            .then(|| offers.part(network_id.as_str(), intents));
        if pay_fees {
            let (fee, shortfall) = match &part {
                Some(part) => {
                    let mock = part.mock_prove().map_err(transfer_err("mock_prove"))?;
                    let merged = external
                        .merge(&mock)
                        .map_err(transfer_err("merge the part"))?;
                    compute_missing_dust(&tx_info, &merged)?
                }
                None => compute_missing_dust(&tx_info, external)?,
            };
            if let Some(dust) = shortfall {
                tracker.record_shortfall(fee, dust);
                continue;
            }
        }
        let Some(part) = part else {
            return Ok(None);
        };
        let mut prepared = PreparedTransfer::new(tx_info, part, spends);
        prepared.set_spent_shielded_inputs(plan.inputs.iter().map(|(n, _)| *n).collect());
        prepared.expires_at = ttl;
        prepared.external = Some(external.clone());
        return Ok(Some(prepared));
    }
    Err(tracker.into_error())
}

/// The shielded deficits of a transaction whose balance is `balance` that
/// `cover` takes, by token and segment.
///
/// A Dust deficit is the fee, which the fee loop covers.
///
/// # Errors
///
/// [`WalletError::Transfer`] for a deficit that `cover` does not take: any
/// shielded deficit for [`Cover::Fee`], and any unshielded deficit. A part of
/// its own cannot cover an unshielded deficit: the ledger counts a fallible one
/// at the segment of its intent, and a merge refuses a second intent there.
fn deficits_to_cover(
    balance: &BTreeMap<(TokenType, u16), i128>,
    cover: &Cover,
) -> Result<BTreeMap<(ShieldedTokenType, u16), u128>, WalletError> {
    let mut deficits = BTreeMap::new();
    for (&(token, segment), &value) in balance {
        if value >= 0 {
            continue;
        }
        let lack = value.unsigned_abs();
        let refused = |what: String, reason: &str| {
            WalletError::Transfer(format!(
                "the transaction lacks {lack} of {what} at segment {segment}, and {reason}"
            ))
        };
        match (token, cover) {
            (TokenType::Dust, _) => {}
            (TokenType::Shielded(token), Cover::Shortfalls { .. }) => {
                deficits.insert((token, segment), lack);
            }
            (TokenType::Shielded(token), Cover::Fee) => {
                return Err(refused(
                    format!("shielded token {}", token.into_sdk()),
                    "a sponsor pays the Dust fee alone",
                ));
            }
            (TokenType::Unshielded(token), _) => {
                return Err(refused(
                    format!("unshielded token {}", token.into_sdk()),
                    "a part of its own cannot cover an unshielded deficit",
                ));
            }
        }
    }
    Ok(deficits)
}

/// A coin in the funding view of a wallet: its nullifier, token and value.
type FreeCoin = (Nullifier, ShieldedTokenType, u128);

/// The coins of `shielded`, the shielded part of a [`Funding`].
fn free_coins(shielded: &WalletState<DefaultDB>) -> Vec<FreeCoin> {
    shielded
        .coins
        .iter()
        .map(|(nullifier, coin)| (nullifier, coin.type_, coin.value))
        .collect()
}

/// The coins that a cover spends, and the change that it returns.
#[derive(Debug, PartialEq, Eq)]
struct CoverPlan {
    /// Each coin to spend, and the segment its input rides at. The pins come
    /// first.
    inputs: Vec<(Nullifier, u16)>,
    /// The change at each token and segment.
    change: Vec<((ShieldedTokenType, u16), u128)>,
}

/// Plan how a cover pays `deficits` from `coins`, the funding view of the
/// wallet.
///
/// Each coin of `pins` is spent whole, at the segment of the largest deficit
/// of its token that is left, or at the guaranteed segment when none is left.
/// Then each deficit that the pins leave takes coins of its token, largest
/// first, for the reason that [`select_largest_first`] gives. The ledger balances
/// each token at each segment, so what the coins at a token and segment hold
/// beyond its deficit comes back as change at that segment.
///
/// # Errors
///
/// [`WalletError::DuplicateInput`] when `pins` names a coin twice,
/// [`WalletError::Transfer`] when a pin is not in `coins`, and
/// [`WalletError::InsufficientShielded`] when the coins of a token do not cover
/// its deficits. That error gives the deficits of the token across segments,
/// and what `coins` hold of it.
fn plan_cover(
    deficits: &BTreeMap<(ShieldedTokenType, u16), u128>,
    pins: &[Nullifier],
    coins: &[FreeCoin],
) -> Result<CoverPlan, WalletError> {
    refuse_repeated_nullifiers(pins)?;
    let mut taken = Taken {
        left: deficits.clone(),
        totals: BTreeMap::new(),
        inputs: Vec::new(),
    };

    for pin in pins {
        let coin = *coins.iter().find(|(n, ..)| n == pin).ok_or_else(|| {
            WalletError::Transfer(format!(
                "shielded coin {pin:?} is not spendable by this wallet \
                 (already spent, not yet synced, or not owned by it)"
            ))
        })?;
        let segment = taken
            .left
            .iter()
            .filter(|((token, _), left)| *token == coin.1 && **left > 0)
            .max_by_key(|(_, left)| **left)
            .map_or(Segment::Guaranteed.into(), |((_, segment), _)| *segment);
        taken.take(coin, segment)?;
    }

    for &key @ (token, segment) in deficits.keys() {
        let mut candidates: Vec<FreeCoin> = coins
            .iter()
            .filter(|(n, t, _)| *t == token && !taken.spends(n))
            .copied()
            .collect();
        candidates.sort_by_key(|(.., value)| Reverse(*value));
        for coin in candidates {
            if taken.left[&key] == 0 {
                break;
            }
            taken.take(coin, segment)?;
        }
        if taken.left[&key] > 0 {
            return Err(WalletError::InsufficientShielded {
                token_type: token.into_sdk(),
                required: deficits
                    .iter()
                    .filter(|((t, _), _)| *t == token)
                    .fold(0u128, |sum, (_, deficit)| sum.saturating_add(*deficit)),
                available: coins
                    .iter()
                    .filter(|(_, t, _)| *t == token)
                    .fold(0u128, |sum, (.., value)| sum.saturating_add(*value)),
            });
        }
    }

    let change = taken
        .totals
        .into_iter()
        .filter_map(|(key, total)| {
            let owed = total.saturating_sub(deficits.get(&key).copied().unwrap_or(0));
            (owed > 0).then_some((key, owed))
        })
        .collect();
    Ok(CoverPlan {
        inputs: taken.inputs,
        change,
    })
}

/// The coins that [`plan_cover`] has taken, and the deficits they leave.
struct Taken {
    left: BTreeMap<(ShieldedTokenType, u16), u128>,
    totals: BTreeMap<(ShieldedTokenType, u16), u128>,
    inputs: Vec<(Nullifier, u16)>,
}

impl Taken {
    /// Spend `coin` at `segment`.
    fn take(
        &mut self,
        (nullifier, token, value): FreeCoin,
        segment: u16,
    ) -> Result<(), WalletError> {
        let total = self.totals.entry((token, segment)).or_default();
        *total = total
            .checked_add(value)
            .ok_or_else(|| WalletError::Transfer("shielded values overflow a u128 total".into()))?;
        if let Some(left) = self.left.get_mut(&(token, segment)) {
            *left = left.saturating_sub(value);
        }
        self.inputs.push((nullifier, segment));
        Ok(())
    }

    fn spends(&self, nullifier: &Nullifier) -> bool {
        self.inputs.iter().any(|(n, _)| n == nullifier)
    }
}

/// The Zswap offers of a cover's part.
///
/// Built once, because each input hands over its spend once.
struct CoverOffers {
    guaranteed: Option<Offer<ProofPreimage, DefaultDB>>,
    fallible: HashMapStorage<u16, Offer<ProofPreimage, DefaultDB>, DefaultDB>,
}

impl CoverOffers {
    fn is_empty(&self) -> bool {
        self.guaranteed.is_none() && self.fallible.is_empty()
    }

    /// The part that carries these offers and `intents`.
    fn part(
        &self,
        network_id: &str,
        intents: HashMapStorage<u16, midnight_helpers::UnprovenIntent<DefaultDB>, DefaultDB>,
    ) -> UnprovenTx {
        Transaction::new(
            network_id,
            intents,
            self.guaranteed.clone(),
            self.fallible.clone(),
        )
    }
}

/// Spend the coins of `plan` from `funding`, and build the offers that carry
/// them and the change.
fn build_cover_offers(
    tx_info: &mut StandardTransactionInfo<DefaultDB, BuildContext>,
    funding: &mut Funding,
    plan: &CoverPlan,
) -> Result<CoverOffers, WalletError> {
    let nullifiers: Vec<Nullifier> = plan.inputs.iter().map(|(n, _)| *n).collect();
    let prepared = prepare_shielded_inputs(
        &mut funding.shielded,
        &funding.shielded_keys,
        &nullifiers,
        &mut tx_info.rng.split(),
    )?;
    let empty = || OfferInfo {
        inputs: Vec::new(),
        outputs: Vec::new(),
        transients: Vec::new(),
    };
    let mut by_segment: BTreeMap<u16, OfferInfo<DefaultDB, BuildContext>> = BTreeMap::new();
    for (input, (_, segment)) in prepared.into_iter().zip(&plan.inputs) {
        let offer = by_segment.entry(*segment).or_insert_with(empty);
        offer.inputs.push(Box::new(input));
    }
    for &((token_type, segment), value) in &plan.change {
        let offer = by_segment.entry(segment).or_insert_with(empty);
        offer.outputs.push(Box::new(OutputInfo {
            destination: funding.own_shielded(),
            token_type,
            value,
        }));
    }

    let guaranteed_segment: u16 = Segment::Guaranteed.into();
    let mut offers = CoverOffers {
        guaranteed: None,
        fallible: HashMapStorage::new(),
    };
    for (segment, mut info) in by_segment {
        let offer = info
            .build(&mut tx_info.rng, tx_info.context.clone())
            .map_err(transfer_err("build the cover offer"))?;
        if segment == guaranteed_segment {
            offers.guaranteed = Some(offer);
        } else {
            offers.fallible = offers.fallible.insert(segment, offer);
        }
    }
    Ok(offers)
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
/// Rounds are side-effect-free: each round rebuilds the candidate from `tx`,
/// so a later round cannot double-spend what an earlier one selected.
fn balance_fees_with_mocks(
    tx_info: &StandardTransactionInfo<DefaultDB, BuildContext>,
    fee: &Fee<'_>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<(UnprovenTx, Spends), WalletError> {
    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let spends = gather_dust_spends(&tx_info.context, fee.dust(), tracker.request(), now)?;
        let mut paid_tx = tx.clone();
        apply_dust(
            &mut paid_tx,
            &spends,
            fee.registration(),
            tx_info.rng.clone().split(),
            ttl,
            now,
        );
        let mock = paid_tx.mock_prove().map_err(transfer_err("mock_prove"))?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &mock)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        return Ok((paid_tx, spends));
    }
    Err(tracker.into_error())
}

/// Dust spends from `dust` that cover `required_amount` at `ctime`, priced
/// with the Dust parameters of `context`. `speculative_spend` leaves `dust` as
/// it is, so each round of a fee loop draws from the same state.
///
/// # Errors
///
/// [`WalletError::InsufficientDust`] when `dust` holds too little, or when
/// there is no `dust` and `required_amount` is above zero.
fn gather_dust_spends(
    context: &BuildContext,
    dust: Option<&DustWallet<DefaultDB>>,
    required_amount: u128,
    ctime: Timestamp,
) -> Result<Spends, WalletError> {
    if required_amount == 0 {
        return Ok(Vec::new());
    }
    let spends = match dust {
        Some(dust) => {
            let params: DustParameters = context.with_ledger_state(|ls| ls.parameters.dust);
            dust.speculative_spend(required_amount, ctime, &params)
                .map_err(transfer_err("speculative_spend"))?
                .0
        }
        None => Vec::new(),
    };
    // Short of the request, `speculative_spend` drew on every unreserved Dust
    // UTXO, so the spends hold all that the wallet can give.
    let provided = held(spends.iter().map(|spend| spend.v_fee));
    if provided < required_amount {
        return Err(WalletError::InsufficientDust {
            required: required_amount,
            available: provided,
        });
    }
    Ok(spends)
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

/// Put `spends` and `registration` in the Dust actions of the intent at the
/// fallible segment of `tx`, which an empty intent with `ttl` takes when `tx`
/// has none there.
///
/// The signing data of an intent covers its Dust actions, so a signature made
/// before them would not verify. The offers and the registration hold
/// placeholders, which the wallet replaces after the proof.
fn apply_dust(
    tx: &mut UnprovenTx,
    spends: &[DustSpend<ProofPreimageMarker, DefaultDB>],
    registration: Option<&DustRegistration<Signature, DefaultDB>>,
    mut rng: StdRng,
    ttl: Timestamp,
    now: Timestamp,
) {
    let Transaction::Standard(stx) = tx else {
        return;
    };

    if spends.is_empty() && registration.is_none() {
        return;
    }

    let segment_id: u16 = Segment::Fallible.into();
    let mut intent = match stx.intents.get(&segment_id) {
        Some(intent) => (*intent).clone(),
        None => Intent::empty(&mut rng, ttl),
    };
    intent.dust_actions = Some(Sp::new(DustActions {
        spends: spends.to_vec().into(),
        registrations: registration.into_iter().cloned().collect::<Vec<_>>().into(),
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

#[cfg(test)]
mod tests {
    /// A fee-only transaction at a segment.
    fn intent_at(segment: u16) -> FinalizedTx {
        use midnight_helpers::SeedableRng;
        let mut rng = StdRng::seed_from_u64(7);
        let intent: midnight_helpers::UnprovenIntent<DefaultDB> =
            Intent::empty(&mut rng, Timestamp::from_secs(1));
        let tx: UnprovenTx =
            Transaction::from_intents("undeployed", HashMapStorage::new().insert(segment, intent));
        tx.mock_prove().expect("mock prove")
    }

    /// An unproven transaction with one empty intent per TTL, in seconds.
    fn unproven_expiring_at(ttls: &[u64]) -> UnprovenTx {
        use midnight_helpers::SeedableRng;
        let mut rng = StdRng::seed_from_u64(7);
        let intents = ttls.iter().zip(1u16..).fold(
            HashMapStorage::<u16, midnight_helpers::UnprovenIntent<DefaultDB>, DefaultDB>::new(),
            |intents, (&ttl, segment)| {
                intents.insert(segment, Intent::empty(&mut rng, Timestamp::from_secs(ttl)))
            },
        );
        Transaction::from_intents("undeployed", intents)
    }

    /// A build context whose block time is `tblock` and whose last block time
    /// is `last_block_time`, both in seconds.
    fn context_at(tblock: u64, last_block_time: u64) -> Arc<BuildContext> {
        let context = BuildContext::new_from_wallet_seeds("undeployed", &[]);
        *context.latest_block_context.lock().unwrap() = Some(BlockContext {
            tblock: Timestamp::from_secs(tblock),
            tblock_err: 30,
            parent_block_hash: Default::default(),
            last_block_time: Timestamp::from_secs(last_block_time),
        });
        Arc::new(context)
    }

    fn global_ttl_of(context: &BuildContext) -> midnight_helpers::Duration {
        context.with_ledger_state(|ls| ls.parameters.global_ttl)
    }

    /// A wallet's block time is its Dust anchor, which can trail the chain
    /// by up to the smaller of `global_ttl` and `dust_grace_period`. A TTL
    /// counted from it reaches the chain already expired when it is shorter
    /// than that lag.
    #[test]
    fn a_ttl_counts_from_the_chain_time_not_the_dust_anchor() {
        let anchor = 1_000_000;
        let chain_now = anchor + 20 * 60;
        let context = context_at(anchor, chain_now);
        assert_eq!(
            intent_ttl(&context, Some(std::time::Duration::from_secs(300))).unwrap(),
            Timestamp::from_secs(chain_now + 300)
        );
    }

    /// The helpers keep the previous block's time as the last block time, or
    /// zero with no block. Their own block time is the chain time, and a TTL
    /// from zero is long expired.
    #[test]
    fn a_context_without_a_later_last_block_counts_from_its_block_time() {
        let context = context_at(1_000_000, 0);
        assert_eq!(
            intent_ttl(&context, None).unwrap(),
            Timestamp::from_secs(1_000_000) + global_ttl_of(&context)
        );
    }

    /// The chain applies an intent whose TTL is `global_ttl` after the block
    /// time, and refuses one later than that, or one that expires before
    /// the block that applies it.
    #[test]
    fn a_ttl_is_a_whole_second_up_to_global_ttl() {
        use std::time::Duration;
        let now = Timestamp::from_secs(1_000);
        let global_ttl = midnight_helpers::Duration::from_secs(3_600);
        let after = |ttl| ttl_after(now, global_ttl, Some(ttl));

        assert_eq!(after(Duration::from_secs(3_600)).unwrap(), now + global_ttl);
        assert_eq!(
            after(Duration::from_millis(1_500)).unwrap(),
            Timestamp::from_secs(1_001),
            "the chain counts whole seconds"
        );
        for refused in [Duration::from_millis(999), Duration::from_secs(3_601)] {
            assert!(
                matches!(
                    after(refused),
                    Err(WalletError::TtlOutOfRange { requested, .. }) if requested == refused
                ),
                "{refused:?} must be refused"
            );
        }
    }

    /// A fee-less shielded transfer has no intent, so its sponsor's fee intent
    /// sets when it expires. Accepting a TTL for it would let it land long
    /// after the caller expects it dead.
    #[test]
    fn a_ttl_needs_an_intent_to_carry_it() {
        let ttl = Some(std::time::Duration::from_secs(60));
        let tx_info = StandardTransactionInfo::new_from_context(
            context_at(1_000, 1_000),
            Arc::new(midnight_helpers::LocalProofServer::default()),
            None,
        );
        assert!(matches!(
            refuse_ttl_without_intent(&tx_info, false, ttl),
            Err(WalletError::TtlWithoutIntent)
        ));
        assert!(refuse_ttl_without_intent(&tx_info, false, None).is_ok());

        // A build that pays its own fee adds a fee intent, which carries it.
        assert!(refuse_ttl_without_intent(&tx_info, true, ttl).is_ok());
    }

    /// A reservation must end when its transaction can no longer land, which
    /// is at the earliest TTL of its intents. With no intent, it gets the
    /// longest TTL from the chain time, not from the Dust anchor.
    #[test]
    fn a_build_expires_with_its_earliest_intent() {
        let context = context_at(1_000, 2_000);
        let prepared = |tx| {
            let tx_info = StandardTransactionInfo::new_from_context(
                context.clone(),
                Arc::new(midnight_helpers::LocalProofServer::default()),
                None,
            );
            PreparedTransfer::new(tx_info, tx, Vec::new())
        };

        assert_eq!(
            prepared(unproven_expiring_at(&[4_000, 3_000])).expires_at(),
            Timestamp::from_secs(3_000)
        );
        assert_eq!(
            prepared(unproven_expiring_at(&[])).expires_at(),
            Timestamp::from_secs(2_000) + global_ttl_of(&context)
        );
    }

    /// The sponsor's Dust must stay reserved no longer than the transaction
    /// it pays for can land, and no longer than the chain's longest TTL,
    /// whatever TTL the other party set. A TTL that already passed must not
    /// make the reservation evictable at the chain time of its build.
    #[test]
    fn a_fee_intent_expires_with_the_transaction_it_pays_for() {
        let context = context_at(1_000, 2_000);
        let longest = Timestamp::from_secs(2_000) + global_ttl_of(&context);
        let fee_ttl_for = |ttls: &[u64]| {
            let external = unproven_expiring_at(ttls).mock_prove().expect("mock prove");
            fee_ttl(&external, &context)
        };

        assert_eq!(fee_ttl_for(&[4_000, 3_000]), Timestamp::from_secs(3_000));
        assert_eq!(fee_ttl_for(&[longest.to_secs() + 1]), longest);
        assert_eq!(fee_ttl_for(&[]), longest);
        assert_eq!(fee_ttl_for(&[1_500]), Timestamp::from_secs(2_000));
    }

    /// A release matches an entry by its input and its stamp. A build that
    /// takes the inputs of an expired build must carry another stamp.
    /// Otherwise the late release of the expired build drops the entry of the
    /// later one. The Dust anchor can stay the same for hours, so a stamp from
    /// it does not move between the two builds.
    #[test]
    fn a_build_after_an_expired_one_carries_a_later_stamp() {
        let anchor = 1_000;
        let t0 = 2_000;
        let prepared = |context, tx| {
            let tx_info = StandardTransactionInfo::new_from_context(
                context,
                Arc::new(midnight_helpers::LocalProofServer::default()),
                None,
            );
            PreparedTransfer::new(tx_info, tx, Vec::new())
        };

        let a = prepared(context_at(anchor, t0), unproven_expiring_at(&[t0 + 60]));
        let b = prepared(
            context_at(anchor, t0 + 120),
            unproven_expiring_at(&[t0 + 180]),
        );
        assert!(b.reserved_at() > a.expires_at());
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

    fn token(byte: u8) -> ShieldedTokenType {
        ShieldedTokenType(midnight_helpers::HashOutput([byte; 32]))
    }

    fn coin(byte: u8) -> Nullifier {
        Nullifier(midnight_helpers::HashOutput([byte; 32]))
    }

    fn shortfalls() -> Cover {
        Cover::Shortfalls {
            pins: Vec::new(),
            pay_fees: true,
        }
    }

    /// A sponsor that took a swap side would pay it from its own coins.
    #[test]
    fn a_sponsor_pays_the_fee_alone() {
        let fee_only = BTreeMap::from([
            ((TokenType::Dust, 0), -500),
            ((TokenType::Shielded(token(1)), 0), 10),
        ]);
        assert!(
            deficits_to_cover(&fee_only, &Cover::Fee)
                .unwrap()
                .is_empty()
        );

        let swap_side = BTreeMap::from([((TokenType::Shielded(token(1)), 1), -10)]);
        assert!(matches!(
            deficits_to_cover(&swap_side, &Cover::Fee),
            Err(WalletError::Transfer(_))
        ));
        assert_eq!(
            deficits_to_cover(&swap_side, &shortfalls()).unwrap(),
            BTreeMap::from([((token(1), 1), 10)])
        );
    }

    /// The ledger balances each token at each segment, so a cover that sums
    /// the deficits of a token leaves one of its segments unbalanced.
    #[test]
    fn the_cover_takes_each_deficit_at_its_segment() {
        let balance = BTreeMap::from([
            ((TokenType::Shielded(token(1)), 0), -5),
            ((TokenType::Shielded(token(1)), 1), -7),
            ((TokenType::Shielded(token(2)), 1), 3),
            ((TokenType::Dust, 0), -100),
        ]);
        assert_eq!(
            deficits_to_cover(&balance, &shortfalls()).unwrap(),
            BTreeMap::from([((token(1), 0), 5), ((token(1), 1), 7)])
        );
    }

    /// A part of its own cannot pay an unshielded deficit, so a cover that
    /// skips one proves a transaction that the node refuses.
    #[test]
    fn no_cover_takes_an_unshielded_deficit() {
        let unshielded =
            midnight_helpers::UnshieldedTokenType(midnight_helpers::HashOutput([3; 32]));
        let balance = BTreeMap::from([((TokenType::Unshielded(unshielded), 0), -1)]);
        for cover in [Cover::Fee, shortfalls()] {
            assert!(
                matches!(
                    deficits_to_cover(&balance, &cover),
                    Err(WalletError::Transfer(_))
                ),
                "{cover:?}"
            );
        }
    }

    /// A pin in the guaranteed offer leaves a fallible receive unbalanced. A
    /// pin with no deficit of its token still comes back whole as change.
    #[test]
    fn a_pin_rides_at_the_segment_of_its_deficit() {
        let deficits = BTreeMap::from([((token(1), 1), 400)]);
        let coins = [(coin(1), token(1), 1000), (coin(2), token(2), 30)];

        assert_eq!(
            plan_cover(&deficits, &[coin(1), coin(2)], &coins).unwrap(),
            CoverPlan {
                inputs: vec![(coin(1), 1), (coin(2), 0)],
                change: vec![((token(1), 1), 600), ((token(2), 0), 30)],
            }
        );
    }

    /// A spend leaves its coin in the funding view, so a selection that does
    /// not skip the pins spends the largest one twice.
    #[test]
    fn the_selection_covers_what_the_pins_leave_with_other_coins() {
        let deficits = BTreeMap::from([((token(1), 0), 12)]);
        let coins = [
            (coin(1), token(1), 8),
            (coin(2), token(1), 5),
            (coin(3), token(1), 3),
            (coin(4), token(2), 50),
        ];

        assert_eq!(
            plan_cover(&deficits, &[coin(1)], &coins).unwrap(),
            CoverPlan {
                inputs: vec![(coin(1), 0), (coin(2), 0)],
                change: vec![((token(1), 0), 1)],
            }
        );
    }

    /// A short cover would prove a transaction that the node refuses. The
    /// error gives what the token needs across its segments and what the
    /// wallet holds of it.
    #[test]
    fn a_short_cover_is_refused_with_the_need_of_the_token() {
        let deficits = BTreeMap::from([((token(1), 0), 6), ((token(1), 1), 4)]);
        let coins = [
            (coin(1), token(1), 3),
            (coin(2), token(1), 4),
            (coin(3), token(2), 100),
        ];

        let err = plan_cover(&deficits, &[], &coins).unwrap_err();
        assert!(
            matches!(
                err,
                WalletError::InsufficientShielded {
                    required: 10,
                    available: 7,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    /// Largest first spends the fewest inputs, and each shielded input adds a
    /// proof to the fee. A selection that took the smallest first would cost
    /// more, and one that stopped short of the need would build a transaction
    /// that the node refuses.
    #[test]
    fn the_selection_takes_the_largest_inputs_until_they_cover_the_need() {
        let candidates = vec![('a', 3), ('b', 10), ('c', 5)];
        assert_eq!(
            select_largest_first(candidates.clone(), 12),
            Some((vec!['b', 'c'], 3))
        );
        assert_eq!(select_largest_first(candidates, 19), None);
    }

    /// The fee of a funding counts a placeholder in each signature slot. A
    /// funding that left the slots empty would price a fee below the one that
    /// the signed transaction owes, and the node would refuse the transaction.
    #[test]
    fn a_transaction_signed_after_its_proof_owes_the_fee_of_one_signed_at_assembly() {
        use midnight_helpers::{SeedableRng, Serializable, UnshieldedWallet};

        let owner =
            UnshieldedWallet::default(crate::hd::Seed::from_bytes(&[5; 32]).unwrap().into_ledger());
        let spend = |byte: u8| UtxoSpend {
            value: 10,
            owner: owner.verifying_key(),
            type_: NIGHT,
            intent_hash: IntentHash(midnight_helpers::HashOutput([byte; 32])),
            output_no: 0,
        };
        let inputs = vec![spend(1), spend(2)];
        let outputs = vec![UtxoOutput {
            value: 20,
            owner: owner.user_address,
            type_: NIGHT,
        }];
        let in_intent = |offer: UnshieldedOffer<Signature, DefaultDB>| {
            let mut intent: UnprovenIntent<DefaultDB> =
                Intent::empty(&mut StdRng::seed_from_u64(7), Timestamp::from_secs(1));
            intent.guaranteed_unshielded_offer = Some(Sp::new(offer));
            intent
        };
        let sealed = |intent: UnprovenIntent<DefaultDB>| {
            let tx: UnprovenTx =
                Transaction::from_intents("undeployed", HashMapStorage::new().insert(1, intent));
            tx.mock_prove().expect("mock prove")
        };
        let fee = |tx: &FinalizedTx| {
            tx.fees_with_margin(&midnight_helpers::INITIAL_PARAMETERS, 3)
                .expect("fee")
        };

        let unsigned = placeholder_offer(inputs.clone(), outputs.clone());
        let at_assembly = in_intent(UnshieldedOffer {
            signatures: Vec::new().into(),
            ..unsigned.clone()
        })
        .sign(
            &mut StdRng::seed_from_u64(1),
            1,
            &[
                owner.transaction_signing_key(),
                owner.transaction_signing_key(),
            ],
            &[],
            &[],
        )
        .expect("sign at assembly");

        let mut after_proof = crate::ProvenTx {
            tx: sealed(in_intent(unsigned)),
            reservation: crate::build::Reservation::new(SpentInputs::default(), |_| {}),
        };
        for request in after_proof.signature_requests() {
            let Signature::Schnorr(signature) =
                owner.sign(&mut StdRng::seed_from_u64(2), &request.data)
            else {
                unreachable!("a default unshielded wallet signs with Schnorr");
            };
            let mut bytes = Vec::new();
            signature.serialize(&mut bytes).unwrap();
            after_proof
                .add_signature(&request, bytes.try_into().unwrap())
                .expect("the signature verifies");
        }
        assert!(after_proof.signature_requests().is_empty());

        assert_eq!(fee(&after_proof.tx), fee(&sealed(at_assembly)));
    }
}
