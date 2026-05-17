use std::sync::Arc;

use midnight_node_ledger_helpers::{
    DefaultDB, FromContext, InputInfo, IntentInfo, OfferInfo, OutputInfo, ProofProvider,
    ShieldedTokenType, StandardTrasactionInfo, UnshieldedOfferInfo, UnshieldedTokenType,
    UtxoOutputInfo, UtxoSpendInfo, WalletSeed,
};

use crate::state::WalletState;
use crate::WalletError;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
}

pub struct TransferBuilder<'a> {
    state: &'a WalletState,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(
        state: &'a WalletState,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            state,
            proof_provider,
        }
    }

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
                .map_err(|e| WalletError::Sync(format!("utxo selection: {e}")))?;

        let mut outputs: Vec<
            Box<dyn midnight_node_ledger_helpers::BuildUtxoOutput<DefaultDB>>,
        > = vec![Box::new(UtxoOutputInfo {
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
                    Box::new(s)
                        as Box<dyn midnight_node_ledger_helpers::BuildUtxoSpend<DefaultDB>>
                })
                .collect(),
            outputs,
        };

        let intent_info: IntentInfo<DefaultDB> = IntentInfo {
            guaranteed_unshielded_offer: Some(unshielded_offer),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        let mut tx_info = StandardTrasactionInfo::new_from_context(
            context,
            self.proof_provider,
            None,
        );
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
        .map_err(|e| WalletError::Sync(format!("prove/balance failed: {e:?}")))?;

    let mut bytes = Vec::new();
    midnight_node_ledger_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
        .map_err(|e| WalletError::Sync(format!("serialize: {e}")))?;

    Ok(TransferResult { tx_bytes: bytes })
}
