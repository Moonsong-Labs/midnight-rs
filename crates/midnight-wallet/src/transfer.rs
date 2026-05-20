use std::collections::VecDeque;
use std::sync::Arc;

use midnight_node_ledger_helpers::{
    BuildUtxoOutput, BuildUtxoSpend, DefaultDB, DustRegistrationBuilder, FromContext, InputInfo,
    IntentInfo, LedgerContext, NIGHT, OfferInfo, OutputInfo, ProofProvider, Segment,
    ShieldedTokenType, StandardTrasactionInfo, Timestamp, UnshieldedOfferInfo, UnshieldedTokenType,
    UnshieldedWallet, UtxoOutputInfo, UtxoSpendInfo, WalletSeed,
};

use crate::WalletError;
use crate::state::WalletState;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
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

        prove_and_serialize(tx_info).await
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
    let finalized = tx_info
        .prove()
        .await
        .map_err(|e| WalletError::Transfer(format!("prove/balance failed: {e:?}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

    Ok(TransferResult { tx_bytes: bytes })
}
