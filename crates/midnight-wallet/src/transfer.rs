use std::sync::Arc;

use midnight_node_ledger_helpers::{
    DefaultDB, FromContext, InputInfo, IntentInfo, OfferInfo, OutputInfo, ProofProvider,
    ShieldedTokenType, StandardTrasactionInfo, UnshieldedOfferInfo, UnshieldedTokenType,
    UtxoOutputInfo, UtxoSpendInfo, WalletSeed,
};

use crate::WalletError;
use crate::state::WalletState;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
}

impl TransferResult {
    /// Submit this transfer transaction to a Midnight node.
    ///
    /// Connects via WebSocket and submits the proven transaction bytes.
    /// Returns the transaction hash on success.
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
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(state: &'a WalletState, proof_provider: Arc<dyn ProofProvider<DefaultDB>>) -> Self {
        Self {
            state,
            proof_provider,
        }
    }

    /// Build a shielded (ZSwap) transfer transaction.
    ///
    /// Spends a shielded coin of the given `token_type` and `amount` from
    /// this wallet and creates an output for `to_seed`.
    pub async fn shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        to_seed: WalletSeed,
    ) -> Result<TransferResult, WalletError> {
        let context = self.state.context().clone();
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
            StandardTrasactionInfo::new_from_context(context, self.proof_provider, None);
        tx_info.set_guaranteed_offer(offer);
        tx_info.set_funding_seeds(vec![from_seed]);
        tx_info.use_mock_proofs_for_fees(false);

        prove_and_serialize(tx_info).await
    }

    /// Build an unshielded (UTXO) transfer transaction.
    ///
    /// Selects UTXOs of the given `token_type` from this wallet to cover
    /// `amount`, sends them to `to_seed`, and returns change to self.
    pub async fn unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        to_seed: WalletSeed,
    ) -> Result<TransferResult, WalletError> {
        let context = self.state.context().clone();
        let from_seed = *self.state.seed();

        let (spend_infos, change) =
            UtxoSpendInfo::utxos_to_cover_value(context.clone(), from_seed, amount, token_type)
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
            StandardTrasactionInfo::new_from_context(context, self.proof_provider, None);
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
