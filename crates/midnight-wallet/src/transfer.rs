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
    /// `mn_shield-addr_undeployed1...`). Only the public material is needed —
    /// the address carries the recipient's `coin_public_key` and
    /// `enc_public_key`, which is all the chain needs to construct the
    /// output coin commitment and encrypt the coin info for them.
    pub async fn shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
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
        tx_info.set_funding_seeds(vec![from_seed]);
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
    pub async fn unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
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
        tx_info.set_funding_seeds(vec![from_seed]);
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
    let built = build_no_validate(tx_info).await?;
    let mut bytes = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

    Ok(TransferResult {
        tx_bytes: bytes,
        spent_unshielded_inputs: Vec::new(),
        dust_batches: built.dust_batches,
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

async fn pay_fees_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<BuiltTransaction, WalletError> {
    let mut missing_dust: u128 = 0;

    for _ in 0..10 {
        let batches = gather_dust_spends(tx_info, missing_dust, now)?;
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
            if let Some(dust) = compute_missing_dust(tx_info, &mock)? {
                missing_dust += dust;
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
        if let Some(dust) = compute_missing_dust(tx_info, &proven)? {
            missing_dust += dust;
            continue;
        }
        confirm_dust_spends(tx_info, &batches)?;
        return Ok(BuiltTransaction {
            finalized: proven,
            dust_batches: batches,
        });
    }
    Err(WalletError::Transfer(
        "could not balance TX after 10 iterations".into(),
    ))
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
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| WalletError::Transfer("wallets lock poisoned".into()))?;
    for seed in &tx_info.funding_seeds {
        if remaining == 0 {
            break;
        }
        let wallet = wallets
            .get_mut(seed)
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

fn compute_missing_dust(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    tx: &FinalizedTx,
) -> Result<Option<u128>, WalletError> {
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
        Ok(Some(dust_imbalance.unsigned_abs()))
    } else {
        Ok(None)
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
