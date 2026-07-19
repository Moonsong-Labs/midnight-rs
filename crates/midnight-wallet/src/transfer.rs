use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;

use midnight_helpers::{
    BuildUtxoOutput, BuildUtxoSpend, CoinSelectionStrategy, DefaultDB, DustActions, DustLocalState,
    DustRegistrationBuilder, DustSpend, FromContext, HashMapStorage, InputInfo, Intent, IntentInfo,
    LedgerContext, NIGHT, OfferInfo, OutputInfo, PedersenRandomness, ProofPreimageMarker,
    ProofProvider, Segment, ShieldedTokenType, ShieldedWallet, Signature, Sp, SplittableRng,
    StandardTrasactionInfo, StdRng, Timestamp, TokenType, Transaction, UnshieldedOfferInfo,
    UnshieldedTokenType, UnshieldedWallet, UtxoOutputInfo, UtxoSpendInfo, WalletAddress,
    WalletSeed,
};

use crate::WalletError;
use crate::state::Wallet;

type UnprovenTx = Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>;
type FinalizedTx = midnight_helpers::FinalizedTransaction<DefaultDB>;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
    /// Unshielded UTXO inputs consumed by this transaction. Pass to
    /// [`crate::Wallet::reserve_pending`] together with `dust_batches` so
    /// subsequent in-process builds don't re-select the same inputs before
    /// the indexer surfaces the spend events.
    pub spent_unshielded_inputs: Vec<SpentUtxoKey>,
    /// Dust batches that funded this transaction's fees. Each batch's
    /// `(spends, updated_state)` pair came from one `speculative_spend`
    /// call and must be kept together for the new `mark_spent` API.
    /// Same caveat as `spent_unshielded_inputs` — pass to
    /// [`crate::Wallet::reserve_pending`] for double-build prevention.
    pub dust_batches: Vec<DustSpendBatch>,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    pub fee_speck: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    pub intent_hash: String,
    pub output_index: u32,
}

pub struct TransferBuilder<'a> {
    state: &'a Wallet,
    context: Arc<LedgerContext<DefaultDB>>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(
        state: &'a Wallet,
        context: Arc<LedgerContext<DefaultDB>>,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            state,
            context,
            proof_provider,
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
    /// When `pay_fees` is false the build skips Dust entirely, yielding a proven
    /// but fee-unbalanced transaction for a multi-party flow where another
    /// wallet pays the fees (the `.without_dust()` path). It is not submittable
    /// on its own; hand it to the fee payer, who completes it with
    /// `MidnightProvider::balance_transaction` (in `midnight-provider`) and
    /// submits.
    pub async fn shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<TransferResult, WalletError> {
        let from_seed = self.state.seed().clone();
        let recipient_wallet = parse_shielded_recipient(recipient)?;

        let input = InputInfo {
            origin: from_seed.clone(),
            token_type,
            value: amount,
            nullifier: None,
        };
        let output: OutputInfo<ShieldedWallet<DefaultDB>> = OutputInfo {
            destination: recipient_wallet,
            token_type,
            value: amount,
        };

        let offer = OfferInfo {
            inputs: vec![Box::new(input)],
            outputs: vec![Box::new(output)],
            transients: vec![],
        };

        let mut tx_info =
            StandardTrasactionInfo::new_from_context(self.context, self.proof_provider, None);
        tx_info.set_guaranteed_offer(offer);
        // Fund the Dust fee from our own seed unless this is a Dustless build
        // (another wallet will sponsor the fees).
        if pay_fees {
            tx_info.set_funding_seeds(vec![from_seed]);
        }
        tx_info.use_mock_proofs_for_fees(false);

        prove_and_serialize(tx_info).await
    }

    /// Build an unshielded (UTXO) transfer transaction.
    ///
    /// `recipient` is a bech32 unshielded address (e.g.
    /// `mn_addr_undeployed1...`). Only the recipient's `user_address` (the
    /// public part) is needed; the chain derives the output's owner field
    /// directly from it. The change output, if any, goes back to the
    /// sender's own seed-derived address.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a proven
    /// but fee-unbalanced transaction for another wallet to sponsor (the
    /// `.without_dust()` path); see [`Self::shielded`] for the multi-party flow.
    pub async fn unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<TransferResult, WalletError> {
        let from_seed = self.state.seed().clone();
        let recipient_wallet = parse_unshielded_recipient(recipient)?;

        let (spend_infos, change) = UtxoSpendInfo::utxos_to_cover_value(
            self.context.clone(),
            from_seed.clone(),
            amount,
            token_type,
            CoinSelectionStrategy::default(),
        )
        .map_err(|e| WalletError::Transfer(format!("utxo selection: {e}")))?;

        let spent_unshielded_inputs: Vec<SpentUtxoKey> = spend_infos
            .iter()
            .filter_map(|s| {
                let intent_hash = s.intent_hash.as_ref()?;
                let output_index = s.output_number?;
                Some(SpentUtxoKey {
                    intent_hash: hex::encode(intent_hash.0.0),
                    output_index,
                })
            })
            .collect();

        let mut outputs: Vec<Box<dyn midnight_helpers::BuildUtxoOutput<DefaultDB>>> =
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
                .map(|s| Box::new(s) as Box<dyn midnight_helpers::BuildUtxoSpend<DefaultDB>>)
                .collect(),
            outputs,
        };

        let intent_info: IntentInfo<DefaultDB> = IntentInfo {
            guaranteed_unshielded_offer: Some(unshielded_offer),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        let mut tx_info =
            StandardTrasactionInfo::new_from_context(self.context, self.proof_provider, None);
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
        tx_info.use_mock_proofs_for_fees(false);

        let mut result = prove_and_serialize(tx_info).await?;
        result.spent_unshielded_inputs = spent_unshielded_inputs;
        Ok(result)
    }

    /// Build a dust address registration transaction.
    ///
    /// Spends and re-creates the wallet's tNIGHT UTXOs while registering
    /// the dust address. Uses "generationless fee availability" (virtual dust
    /// accrued by holding tNIGHT) to self-fund the registration fee.
    ///
    /// `utxo_ctime` is the creation timestamp (seconds since epoch) of the
    /// wallet's tNIGHT UTXOs. If `None`, uses `now - 1 hour` as a
    /// conservative estimate.
    pub async fn register_dust(
        self,
        utxo_ctime: Option<u64>,
    ) -> Result<TransferResult, WalletError> {
        let seed = self.state.seed().clone();
        let night_hex = "0".repeat(64);

        let night_utxos: Vec<_> = self
            .state
            .unshielded_utxos()
            .iter()
            .filter(|u| u.token_type == night_hex)
            .collect();

        if night_utxos.is_empty() {
            return Err(WalletError::Transfer(
                "no tNIGHT UTXOs available for dust registration".into(),
            ));
        }

        let mut inputs: VecDeque<Box<dyn BuildUtxoSpend<DefaultDB>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoSpendInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                    intent_hash: utxo
                        .intent_hash
                        .as_deref()
                        .and_then(crate::state::parse_intent_hash_hex),
                    output_number: utxo.output_index.map(|i| i as u32),
                };
                Box::new(info) as Box<dyn BuildUtxoSpend<DefaultDB>>
            })
            .collect();

        let mut outputs: VecDeque<Box<dyn BuildUtxoOutput<DefaultDB>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoOutputInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                };
                Box::new(info) as Box<dyn BuildUtxoOutput<DefaultDB>>
            })
            .collect();

        let guaranteed_inputs = inputs.pop_front().into_iter().collect();
        let guaranteed_outputs = outputs.pop_front().into_iter().collect();
        let guaranteed = UnshieldedOfferInfo {
            inputs: guaranteed_inputs,
            outputs: guaranteed_outputs,
        };

        let fallible = if !inputs.is_empty() {
            Some(UnshieldedOfferInfo {
                inputs: inputs.into(),
                outputs: outputs.into(),
            })
        } else {
            None
        };

        let intent = IntentInfo {
            guaranteed_unshielded_offer: Some(guaranteed),
            fallible_unshielded_offer: fallible,
            actions: vec![],
        };

        let dust_params = &self.state.parameters().dust;
        let now = self.context.latest_block_context().tblock;
        let ctime = match utxo_ctime {
            Some(t) => Timestamp::from_secs(t),
            None => Timestamp::from_secs(now.to_secs().saturating_sub(3600)),
        };
        let allow_fee_payment = generationless_fee_availability(
            &night_utxos.iter().map(|u| u.value).collect::<Vec<_>>(),
            dust_params.night_dust_ratio,
            dust_params.generation_decay_rate,
            now,
            ctime,
        );

        let unshielded = UnshieldedWallet::default(seed.clone());
        let signing_key = unshielded.signing_key().clone();
        let dust_public_key = self.state.dust_wallet().public_key;

        let mut tx_info = StandardTrasactionInfo::new_from_context(
            self.context.clone(),
            self.proof_provider.clone(),
            None,
        );
        tx_info.add_intent(Segment::Fallible.into(), Box::new(intent));
        tx_info.add_dust_registration(DustRegistrationBuilder {
            signing_key,
            dust_address: Some(dust_public_key),
            allow_fee_payment,
        });
        tx_info.use_mock_proofs_for_fees(true);

        // Registration spends all tNIGHT UTXOs (one per offer leg). Capture
        // their keys so callers can avoid re-selecting them via
        // `Wallet::remove_unshielded_spent` before the indexer confirms.
        let spent_unshielded_inputs: Vec<SpentUtxoKey> = night_utxos
            .iter()
            .filter_map(|u| {
                Some(SpentUtxoKey {
                    intent_hash: u.intent_hash.clone()?,
                    output_index: u.output_index? as u32,
                })
            })
            .collect();

        let mut result = prove_and_serialize(tx_info).await?;
        result.spent_unshielded_inputs = spent_unshielded_inputs;
        Ok(result)
    }
}

fn generationless_fee_availability(
    utxo_values: &[u128],
    night_dust_ratio: u64,
    generation_decay_rate: u32,
    now: Timestamp,
    ctime: Timestamp,
) -> u128 {
    let dt = u128::try_from((now - ctime).as_seconds()).unwrap_or(0);
    utxo_values
        .iter()
        .map(|&value| {
            let vfull = value.saturating_mul(night_dust_ratio as u128);
            let rate = value.saturating_mul(generation_decay_rate as u128);
            u128::min(dt.saturating_mul(rate), vfull)
        })
        .fold(0u128, |a, b| a.saturating_add(b))
}

async fn prove_and_serialize(
    tx_info: StandardTrasactionInfo<DefaultDB>,
) -> Result<TransferResult, WalletError> {
    // `build_no_validate` consumes `tx_info`. Keep a handle to the ledger
    // context so we can read `parameters` after the build to compute the fee.
    // `Arc::clone` is cheap and the lock inside `with_ledger_state` is held
    // only for the duration of the closure.
    let context = tx_info.context.clone();
    let built = build_no_validate(tx_info).await?;
    let mut bytes = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

    // Mirrors the node's own estimation RPC: `enforce_time_to_dismiss = false`,
    // i.e. report the deterministic SPECK cost without the chain-side
    // mempool-eviction check. The chain charges the same number at inclusion;
    // if the tx exceeds the eviction-time bound, that surfaces at submit, not
    // here.
    let fee_speck = context
        .with_ledger_state(|s| built.finalized.fees(&s.parameters, false))
        .map_err(transfer_err("fees"))?;

    Ok(TransferResult {
        tx_bytes: bytes,
        spent_unshielded_inputs: Vec::new(),
        dust_batches: built.dust_batches,
        fee_speck,
    })
}

fn transfer_err<E: std::fmt::Debug>(ctx: &str) -> impl FnOnce(E) -> WalletError + '_ {
    move |e| WalletError::Transfer(format!("{ctx}: {e:?}"))
}

/// The proven transaction plus the dust batches that funded it.
///
/// Each [`DustSpendBatch`] groups per-seed `(spends, updated_state)` from a
/// single `speculative_spend` call, since the new helpers `mark_spent` API
/// requires that pair together. Callers pass these batches to
/// [`crate::Wallet::reserve_pending`] so subsequent in-process builds
/// (before the indexer surfaces the spend events) don't re-select the same
/// dust UTXOs.
pub struct BuiltTransaction {
    pub finalized: FinalizedTx,
    pub dust_batches: Vec<DustSpendBatch>,
}

/// Build and prove a transaction without the helpers' final `well_formed()`
/// check. The chain validates with its own `root_history`; matching that
/// locally would require a 55MB+ global `DustState`. Matches midnight-js.
pub async fn build_no_validate(
    mut tx_info: StandardTrasactionInfo<DefaultDB>,
) -> Result<BuiltTransaction, WalletError> {
    let now = tx_info.context.latest_block_context().tblock;
    let delay = tx_info
        .context
        .with_ledger_state(|ls| ls.parameters.global_ttl);
    let ttl = now + delay;

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

    let mut intents = HashMapStorage::<
        u16,
        Intent<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        DefaultDB,
    >::new();
    for (segment_id, intent_info) in tx_info.intents.iter_mut() {
        let intent = intent_info
            .build(&mut tx_info.rng, ttl, tx_info.context.clone(), *segment_id)
            .await;
        intents = intents.insert(*segment_id, intent);
    }

    let network_id = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?
        .network_id
        .clone();

    let tx = Transaction::new(network_id, intents, guaranteed_offer, fallible_offer);

    if tx_info.funding_seeds.is_empty() && tx_info.dust_registrations.is_empty() {
        let finalized = prove_tx_no_validate(&mut tx_info, tx).await?;
        Ok(BuiltTransaction {
            finalized,
            dust_batches: Vec::new(),
        })
    } else {
        pay_fees_no_validate(&mut tx_info, tx, now, ttl).await
    }
}

/// Upper bound on fee-balancing rounds in [`pay_fees_no_validate`]. Each
/// round requests the candidate's full dust need, so in practice the loop
/// converges in 2-3 rounds; hitting the cap means the fee keeps growing
/// faster than the spends added to pay it.
const MAX_FEE_BALANCE_ITERATIONS: usize = 10;

/// Convergence bookkeeping for the fee-balancing loop in
/// [`pay_fees_no_validate`].
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
    last_fee: Option<u128>,
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
        self.last_fee = Some(fee);
    }

    fn into_error(self) -> WalletError {
        let fee = self
            .last_fee
            .map_or_else(|| "unknown".to_string(), |f| f.to_string());
        WalletError::Transfer(format!(
            "could not balance TX after {} iterations: last candidate needed {} specks of dust in total (last computed fee {} specks)",
            self.iterations, self.missing_dust, fee
        ))
    }
}

async fn pay_fees_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<BuiltTransaction, WalletError> {
    // Iterations are side-effect-free: `gather_dust_spends` only calls
    // `DustWallet::speculative_spend`, which takes `&self` and clones the
    // local state instead of writing it back, and each round rebuilds
    // `paid_tx` from the original `tx`. The only wallet mutation is
    // `mark_spent`, reached exclusively through `confirm_dust_spends` on
    // the success paths below; it must never move inside the loop, or a
    // retry after an unbalanced round would double-spend the dust the
    // failed round selected.
    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let batches = gather_dust_spends(tx_info, tracker.request(), now)?;
        let flat_spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>> = batches
            .iter()
            .flat_map(|b| b.spends.iter().cloned())
            .collect();
        let mut paid_tx = tx.clone();
        apply_dust(
            tx_info,
            &mut paid_tx,
            &flat_spends,
            tx_info.rng.clone().split(),
            ttl,
            now,
        );

        // Probe with mock proofs (when allowed) to avoid running the
        // expensive ZK prover on iterations that turn out unbalanced.
        if tx_info.mock_proofs_for_fees {
            let mock = paid_tx.mock_prove().map_err(transfer_err("mock_prove"))?;
            let (fee, shortfall) = compute_missing_dust(tx_info, &mock)?;
            if let Some(dust) = shortfall {
                tracker.record_shortfall(fee, dust);
                continue;
            }
            confirm_dust_spends(tx_info, &batches)?;
            let finalized = prove_tx_no_validate(tx_info, paid_tx).await?;
            return Ok(BuiltTransaction {
                finalized,
                dust_batches: batches,
            });
        }
        let proven = prove_tx_no_validate(tx_info, paid_tx).await?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &proven)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        confirm_dust_spends(tx_info, &batches)?;
        return Ok(BuiltTransaction {
            finalized: proven,
            dust_batches: batches,
        });
    }
    Err(tracker.into_error())
}

async fn prove_tx_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
) -> Result<FinalizedTx, WalletError> {
    let resolver = tx_info.context.resolver().await;
    let parameters = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?
        .parameters
        .clone();
    let mut rng = tx_info.rng.split();
    Ok(tx_info
        .prover
        .prove(
            tx,
            rng.split(),
            &resolver,
            &parameters.cost_model.runtime_cost_model,
        )
        .await
        .seal(rng))
}

/// One funding-seed's dust contribution to a transaction.
///
/// `speculative_spend` returns both the spend records and the resulting
/// `DustLocalState`; the new helpers API requires that pair to be passed
/// together to `DustWallet::mark_spent`. We keep them grouped here so
/// callers (and the pending-reservations layer) can preserve the
/// invariant: the same `(spends, updated_state)` produced by a single
/// `speculative_spend` must be applied together.
#[derive(Clone)]
pub struct DustSpendBatch {
    pub seed: WalletSeed,
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    pub updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB>,
}

fn gather_dust_spends(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    required_amount: u128,
    ctime: Timestamp,
) -> Result<Vec<DustSpendBatch>, WalletError> {
    let mut batches: Vec<DustSpendBatch> = Vec::new();
    let mut remaining = required_amount;
    let state = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?;
    let params = &state.parameters.dust;
    let wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| WalletError::Transfer("wallets lock poisoned".into()))?;
    for seed in &tx_info.funding_seeds {
        if remaining == 0 {
            break;
        }
        // `get`, not `get_mut`: gathering must not mutate wallet state.
        // The fee-balancing retries in `pay_fees_no_validate` rely on
        // `speculative_spend` (`&self`) leaving the wallet untouched until
        // `confirm_dust_spends` applies the chosen batch via `mark_spent`.
        let wallet = wallets
            .get(seed)
            .ok_or_else(|| WalletError::Transfer("unrecognized wallet seed".into()))?;
        let (new_spends, updated_state) = wallet
            .dust
            .speculative_spend(remaining, ctime, params)
            .map_err(transfer_err("speculative_spend"))?;
        for spend in &new_spends {
            remaining = remaining.saturating_sub(spend.v_fee);
        }
        batches.push(DustSpendBatch {
            seed: seed.clone(),
            spends: new_spends,
            updated_state,
        });
    }
    if remaining > 0 {
        Err(WalletError::Transfer(format!(
            "insufficient DUST (trying to spend {required_amount}, need {remaining} more)"
        )))
    } else {
        Ok(batches)
    }
}

fn confirm_dust_spends(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    batches: &[DustSpendBatch],
) -> Result<(), WalletError> {
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| WalletError::Transfer("wallets lock poisoned".into()))?;
    for batch in batches {
        if let Some(wallet) = wallets.get_mut(&batch.seed) {
            wallet
                .dust
                .mark_spent(&batch.spends, batch.updated_state.clone());
        }
    }
    Ok(())
}

/// Price a candidate transaction. Returns the fee (with margin, in
/// specks) and, if the candidate doesn't balance, the dust shortfall.
fn compute_missing_dust(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
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
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    tx: &mut UnprovenTx,
    spends: &[midnight_helpers::DustSpend<ProofPreimageMarker, DefaultDB>],
    mut rng: StdRng,
    ttl: Timestamp,
    now: Timestamp,
) {
    let Transaction::Standard(stx) = tx else {
        return;
    };

    if spends.is_empty() && tx_info.dust_registrations.is_empty() {
        return;
    }

    let segment_id: u16 = Segment::Fallible.into();
    let mut intent = match stx.intents.get(&segment_id) {
        Some(intent) => (*intent).clone(),
        None => Intent::empty(&mut rng, ttl),
    };
    let registrations = tx_info
        .dust_registrations
        .iter()
        .map(|registration| registration.build(&intent, &mut rng, segment_id))
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

fn parse_wallet_address(s: &str) -> Result<WalletAddress, WalletError> {
    WalletAddress::from_str(s)
        .map_err(|e| WalletError::InvalidAddress(format!("bech32 decode: {e}")))
}

fn parse_unshielded_recipient(s: &str) -> Result<UnshieldedWallet, WalletError> {
    let addr = parse_wallet_address(s)?;
    UnshieldedWallet::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not an unshielded address: {e:?}")))
}

/// Decode a `mn_shield-addr_*` bech32 string into a typed recipient suitable
/// for use as `OutputInfo::destination` when hand-building a shielded
/// [`OfferInfo`].
pub fn parse_shielded_recipient(s: &str) -> Result<ShieldedWallet<DefaultDB>, WalletError> {
    let addr = parse_wallet_address(s)?;
    ShieldedWallet::<DefaultDB>::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not a shielded address: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The converging path of the fee-balancing loop (tracker is consulted
    // for the request, success returns without touching it) is exercised
    // end-to-end by the devnet integration tests (`build_shielded_transfer`
    // et al. in tests/integration.rs), which run the loop to convergence.

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

    #[test]
    fn error_without_attempts_does_not_fabricate_a_fee() {
        let WalletError::Transfer(msg) = FeeBalanceTracker::default().into_error() else {
            panic!("expected WalletError::Transfer");
        };
        assert!(msg.contains("0 iterations"), "{msg}");
        assert!(msg.contains("unknown"), "{msg}");
    }
}
