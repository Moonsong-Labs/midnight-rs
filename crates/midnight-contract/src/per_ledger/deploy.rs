//! A contract deploy on this generation.

use std::sync::Arc;

use midnight_typed_state::{ContractState, InMemoryDB};

use super::helpers;
use super::provider::Builds;
use super::types::convert::IntoSdk;
use helpers::{
    BuildContext, BuildContractAction, ContractDeploy, DefaultDB, FromContext, IntentInfo,
    OfferInfo, StandardTransactionInfo,
};

use crate::deploy::DeployResult;
use crate::error::ContractError;

/// Deploy `initial_state` with Dust fee payment from the provider's funded
/// wallet. See [`crate::deploy::deploy_funded`].
pub(crate) async fn deploy_funded(
    builds: &Builds<'_>,
    initial_state: &ContractState<InMemoryDB>,
    shielded_offer: Option<OfferInfo<DefaultDB, BuildContext>>,
) -> Result<DeployResult, ContractError> {
    let context = builds.execution_context().await?;
    let state = super::state::from_view(initial_state)?;

    let deploy = ContractDeploy::new(&mut rand::thread_rng(), state);
    let address = deploy.address().into_sdk();

    struct DeployAction {
        deploy: ContractDeploy<DefaultDB>,
    }

    #[async_trait::async_trait]
    impl<C: helpers::BuilderContext<DefaultDB>> BuildContractAction<DefaultDB, C> for DeployAction {
        async fn build(
            &mut self,
            _rng: &mut helpers::StdRng,
            _context: Arc<C>,
            intent: &helpers::Intent<
                helpers::Signature,
                helpers::ProofPreimageMarker,
                helpers::PedersenRandomness,
                DefaultDB,
            >,
        ) -> helpers::Intent<
            helpers::Signature,
            helpers::ProofPreimageMarker,
            helpers::PedersenRandomness,
            DefaultDB,
        > {
            intent.add_deploy(self.deploy.clone())
        }
    }

    let intent_info: IntentInfo<DefaultDB, BuildContext> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(DeployAction { deploy })],
    };

    let mut tx_info =
        StandardTransactionInfo::new_from_context(context, builds.proof_provider(), None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(shielded_offer.unwrap_or_else(|| OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    }));
    tx_info.use_mock_proofs_for_fees(true);

    let built = builds
        .build_funded(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    Ok(DeployResult {
        address,
        tx_bytes: built.tx_bytes,
    })
}
