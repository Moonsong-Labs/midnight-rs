use std::collections::VecDeque;
use std::sync::Arc;

use midnight_node_ledger_helpers::{
    BuildUtxoOutput, BuildUtxoSpend, DefaultDB, DustActions, DustRegistrationBuilder, FromContext,
    HashMapStorage, InputInfo, Intent, IntentInfo, LedgerContext, NIGHT, OfferInfo, OutputInfo,
    PedersenRandomness, ProofPreimageMarker, ProofProvider, Segment, Signature, ShieldedTokenType,
    Sp, SplittableRng, StandardTrasactionInfo, StdRng, Timestamp, TokenType, Transaction,
    UnshieldedOfferInfo, UnshieldedTokenType, UnshieldedWallet, UtxoOutputInfo, UtxoSpendInfo,
    WalletSeed,
};

use crate::WalletError;
use crate::state::WalletState;

type UnprovenTx = Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>;
type FinalizedTx = midnight_node_ledger_helpers::FinalizedTransaction<DefaultDB>;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
    /// Unshielded UTXO inputs consumed by this transaction.
    /// Caller should remove these from local state to avoid double-spending
    /// before the indexer publishes the confirmation events.
    pub spent_unshielded_inputs: Vec<SpentUtxoKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    pub intent_hash: String,
    pub output_index: u32,
}

impl TransferResult {
    /// Submit this transfer transaction to a Midnight node.
    pub async fn submit(&self, node_url: &str) -> Result<String, WalletError> {
        use subxt::{OnlineClient, SubstrateConfig};

        let client = OnlineClient::<SubstrateConfig>::from_insecure_url(node_url)
            .await
            .map_err(|e| WalletError::Submission(format!("connect: {e}")))?;

        let call = subxt::dynamic::tx(
            "Midnight",
            "send_mn_transaction",
            vec![subxt::dynamic::Value::from_bytes(&self.tx_bytes)],
        );

        let tx_client = client
            .tx()
            .await
            .map_err(|e| WalletError::Submission(format!("tx client: {e}")))?;
        let unsigned = tx_client
            .create_unsigned(&call)
            .map_err(|e| WalletError::Submission(format!("create unsigned: {e}")))?;
        let hash = unsigned
            .submit()
            .await
            .map_err(|e| WalletError::Submission(format!("submit: {e}")))?;

        Ok(format!("{hash:?}"))
    }
}

pub struct TransferBuilder<'a> {
    state: &'a WalletState,
    context: Arc<LedgerContext<DefaultDB>>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(
        state: &'a WalletState,
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
    pub async fn shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        to_seed: WalletSeed,
    ) -> Result<TransferResult, WalletError> {
        let from_seed = *self.state.seed();

        let input = InputInfo {
            origin: from_seed,
            token_type,
            value: amount,
        };
        let output = OutputInfo {
            destination: to_seed,
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
    pub async fn unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        to_seed: WalletSeed,
    ) -> Result<TransferResult, WalletError> {
        let from_seed = *self.state.seed();

        let (spend_infos, change) = UtxoSpendInfo::utxos_to_cover_value(
            self.context.clone(),
            from_seed,
            amount,
            token_type,
        )
        .map_err(|e| WalletError::Transfer(format!("utxo selection: {e}")))?;

        // Capture the UTXO keys we're spending so the caller can remove them
        // from local state before the indexer publishes confirmation events.
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

        let mut outputs: Vec<Box<dyn midnight_node_ledger_helpers::BuildUtxoOutput<DefaultDB>>> =
            vec![Box::new(UtxoOutputInfo {
                value: amount,
                owner: to_seed,
                token_type,
            })];

        if change > 0 {
            outputs.push(Box::new(UtxoOutputInfo {
                value: change,
                owner: from_seed,
                token_type,
            }));
        }

        let unshielded_offer = UnshieldedOfferInfo {
            inputs: spend_infos
                .into_iter()
                .map(|s| {
                    Box::new(s) as Box<dyn midnight_node_ledger_helpers::BuildUtxoSpend<DefaultDB>>
                })
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
        let seed = *self.state.seed();
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
                    owner: seed,
                    token_type: NIGHT,
                    intent_hash: utxo.intent_hash.as_deref().and_then(parse_intent_hash),
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
                    owner: seed,
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
        let now = self
            .context
            .latest_block_context()
            .tblock;
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

        let unshielded = UnshieldedWallet::default(seed);
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

        prove_and_serialize(tx_info).await
    }
}

fn parse_intent_hash(hex: &str) -> Option<midnight_node_ledger_helpers::IntentHash> {
    let bytes = hex::decode(hex).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(midnight_node_ledger_helpers::IntentHash(
        midnight_node_ledger_helpers::HashOutput(arr),
    ))
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
    // We don't call StandardTrasactionInfo::prove() because it calls
    // validate() / well_formed() which requires a full DustState with
    // up-to-date root_history. We don't maintain that (we'd need 55MB+
    // of in-memory state per the JS SDK reference). Instead we replicate
    // the helpers' build/pay_fees/prove_tx flow without validation,
    // matching the JS SDK's "build + submit, let the chain validate"
    // pattern.
    let finalized = build_no_validate(tx_info)
        .await
        .map_err(|e| WalletError::Transfer(format!("prove/balance failed: {e}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

    Ok(TransferResult {
        tx_bytes: bytes,
        spent_unshielded_inputs: Vec::new(),
    })
}

/// Build and prove a transaction without calling `validate()` / `well_formed()`.
///
/// Replicates `StandardTrasactionInfo::prove()` from
/// `midnight-node-ledger-helpers` minus the final `tx.well_formed(...)` call.
/// That call requires a `LedgerState` with a `DustState` whose `root_history`
/// matches the chain's current state, which would require maintaining a 55MB+
/// global DustState in memory.
///
/// The JS SDK (Lace wallet) takes the same approach: build the transaction
/// from the local `DustLocalState`, sign and prove, then submit. The chain
/// performs its own validation.
async fn build_no_validate(
    mut tx_info: StandardTrasactionInfo<DefaultDB>,
) -> Result<FinalizedTx, String> {
    let now = tx_info.context.latest_block_context().tblock;
    let delay = tx_info
        .context
        .with_ledger_state(|ls| ls.parameters.global_ttl);
    let ttl = now + delay;

    // Build guaranteed offer
    let guaranteed_offer = match tx_info.guaranteed_offer.as_mut() {
        Some(gc) => Some(
            gc.build(&mut tx_info.rng, tx_info.context.clone())
                .map_err(|e| format!("build guaranteed offer: {e:?}"))?,
        ),
        None => None,
    };

    // Build fallible offers
    let mut fallible_offers_vec = Vec::new();
    for (segment_id, offer_info) in tx_info.fallible_offers.iter_mut() {
        let offer = offer_info
            .build(&mut tx_info.rng, tx_info.context.clone())
            .map_err(|e| format!("build fallible offer: {e:?}"))?;
        fallible_offers_vec.push((*segment_id, offer));
    }
    let fallible_offer = fallible_offers_vec.into_iter().collect();

    // Build intents
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
        .map_err(|_| "ledger state lock was poisoned".to_string())?
        .network_id
        .clone();

    let tx = Transaction::new(network_id, intents, guaranteed_offer, fallible_offer);

    if tx_info.funding_seeds.is_empty() && tx_info.dust_registrations.is_empty() {
        prove_tx_no_validate(&mut tx_info, tx).await
    } else {
        pay_fees_no_validate(&mut tx_info, tx, now, ttl).await
    }
}

async fn pay_fees_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<FinalizedTx, String> {
    let mut missing_dust: u128 = 0;

    for _ in 0..10 {
        let spends = gather_dust_spends(tx_info, missing_dust, now)?;
        let mut paid_tx = tx.clone();
        apply_dust(tx_info, &mut paid_tx, &spends, tx_info.rng.clone().split(), ttl, now);

        if tx_info.mock_proofs_for_fees {
            let mock_proven = paid_tx
                .mock_prove()
                .map_err(|e| format!("mock_prove: {e:?}"))?;
            if let Some(dust) = compute_missing_dust(tx_info, &mock_proven)? {
                missing_dust += dust;
            } else {
                confirm_dust_spends(tx_info, &spends)?;
                return prove_tx_no_validate(tx_info, paid_tx).await;
            }
        } else {
            let proven = prove_tx_no_validate(tx_info, paid_tx).await?;
            if let Some(dust) = compute_missing_dust(tx_info, &proven)? {
                missing_dust += dust;
            } else {
                confirm_dust_spends(tx_info, &spends)?;
                return Ok(proven);
            }
        }
    }
    Err("Could not balance TX".into())
}

async fn prove_tx_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
) -> Result<FinalizedTx, String> {
    let resolver = tx_info.context.resolver().await;
    let parameters = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| "ledger state lock was poisoned".to_string())?
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

fn gather_dust_spends(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    required_amount: u128,
    ctime: Timestamp,
) -> Result<
    Vec<midnight_node_ledger_helpers::DustSpend<ProofPreimageMarker, DefaultDB>>,
    String,
> {
    let mut spends = vec![];
    let mut remaining = required_amount;
    let state = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| "ledger state lock was poisoned".to_string())?;
    let params = &state.parameters.dust;
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| "wallet lock was poisoned".to_string())?;
    for seed in &tx_info.funding_seeds {
        if remaining == 0 {
            return Ok(spends);
        }
        let wallet = wallets
            .get_mut(seed)
            .ok_or_else(|| "Unrecognized wallet seed".to_string())?;
        let new_spends = wallet
            .dust
            .speculative_spend(remaining, ctime, params)
            .map_err(|e| format!("speculative_spend: {e:?}"))?;
        for spend in new_spends {
            remaining -= spend.v_fee;
            spends.push(spend);
        }
    }
    if remaining > 0 {
        Err(format!(
            "Insufficient DUST (trying to spend {required_amount}, need {remaining} more)"
        ))
    } else {
        Ok(spends)
    }
}

fn confirm_dust_spends(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    spends: &[midnight_node_ledger_helpers::DustSpend<ProofPreimageMarker, DefaultDB>],
) -> Result<(), String> {
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| "wallet lock was poisoned".to_string())?;
    for wallet in wallets.values_mut() {
        wallet.dust.mark_spent(spends);
    }
    Ok(())
}

fn compute_missing_dust(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    tx: &FinalizedTx,
) -> Result<Option<u128>, String> {
    let fees = tx_info
        .context
        .with_ledger_state(|s| tx.fees_with_margin(&s.parameters, 3))
        .map_err(|e| format!("fees_with_margin: {e:?}"))?;
    let imbalances = tx
        .balance(Some(fees))
        .map_err(|e| format!("balance: {e:?}"))?;
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
    spends: &[midnight_node_ledger_helpers::DustSpend<ProofPreimageMarker, DefaultDB>],
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
