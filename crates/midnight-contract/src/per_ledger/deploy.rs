//! A contract deploy on this generation.

use std::collections::BTreeMap;
use std::sync::Arc;

use midnight_provider::ProviderError;
use midnight_typed_state::{ContractState, InMemoryDB};

use super::helpers;
use super::provider::Builds;
use super::shielded::{KeyedOutput, change_due};
use super::types::convert::{IntoLedger, IntoSdk};
use helpers::{
    BuildContext, BuildContractAction, BuildInput, BuildOutput, ContractDeploy, DefaultDB,
    FromContext, IntentInfo, OfferInfo, SplittableRng, StandardTransactionInfo, TokenInfo,
};

use crate::ShieldedOffer;
use crate::deploy::DeployResult;
use crate::error::ContractError;
use crate::offer::{refuse_shortfall, value_by_token};

/// Deploy `initial_state` with Dust fee payment from the provider's funded
/// wallet. See [`crate::deploy::deploy_funded`].
pub(crate) async fn deploy_funded(
    builds: &Builds<'_>,
    initial_state: &ContractState<InMemoryDB>,
    shielded_offer: Option<&ShieldedOffer>,
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
        StandardTransactionInfo::new_from_context(context.clone(), builds.proof_provider(), None);
    tx_info.add_intent(1, Box::new(intent_info));

    let tx_bytes = match shielded_offer {
        None => {
            tx_info.set_guaranteed_offer(OfferInfo {
                inputs: vec![],
                outputs: vec![],
                transients: vec![],
            });
            tx_info.use_mock_proofs_for_fees(true);
            builds.build_funded(tx_info).await?.tx_bytes
        }
        Some(offer) => Box::pin(offer_transaction(builds, &context, tx_info, offer)).await?,
    };

    Ok(DeployResult { address, tx_bytes })
}

/// Add `offer` to the deploy in `tx_info`, prove it, and pay its fee.
///
/// The wallet spends the offer's coins into `context` and reserves them. The
/// deploy then proves with no fee, and the wallet pays the Dust fee in an
/// intent of its own, as it does for a circuit call. `build_funded` cannot
/// pay it: it funds `context` again, and the wallet refuses a second funding
/// of one context.
///
/// The coins stay reserved once the fee is paid, and go back to the wallet
/// when a step fails.
async fn offer_transaction(
    builds: &Builds<'_>,
    context: &Arc<BuildContext>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
) -> Result<Vec<u8>, ContractError> {
    let (change_cpk, change_epk) = builds.provider().shielded_public_keys().await?;

    // The spends need the funding view in the context. Reserving before it
    // would hide these coins from it.
    builds.add_funding(context).await?;
    let (prepared, mut held) = builds
        .prepare_shielded_inputs(context, &offer.coins, &mut tx_info.rng.split())
        .await?;

    let held_value = value_by_token(
        prepared
            .iter()
            .map(|input| (input.token_type().into_sdk(), input.value())),
    )?;
    let paid_value = value_by_token(
        offer
            .outputs
            .iter()
            .map(|payment| (payment.token_type, payment.value)),
    )?;
    refuse_shortfall(&held_value, &paid_value)?;

    let payments = offer.outputs.iter().map(|payment| KeyedOutput {
        coin_public_key: payment.recipient.coin_public_key.into_ledger(),
        enc_public_key: payment.recipient.enc_public_key.into_ledger(),
        token_type: payment.token_type.into_ledger(),
        value: payment.value,
    });
    let change = change_due(&held_value, &paid_value, &BTreeMap::new())
        .into_iter()
        .map(|(token_type, value)| KeyedOutput {
            coin_public_key: change_cpk.into_ledger(),
            enc_public_key: change_epk.into_ledger(),
            token_type: token_type.into_ledger(),
            value,
        });
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: prepared
            .into_iter()
            .map(|input| Box::new(input) as Box<dyn BuildInput<DefaultDB, BuildContext>>)
            .collect(),
        outputs: payments
            .chain(change)
            .map(|output| Box::new(output) as Box<dyn BuildOutput<DefaultDB, BuildContext>>)
            .collect(),
        transients: vec![],
    });

    let built = super::types::build_no_validate(tx_info)
        .await
        .map_err(ProviderError::Wallet)?;
    let mut bytes = Vec::new();
    helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;
    // Dropped before the next await, and the fee step boxed, to keep this
    // future small enough for a debug build's stack, as in `call_transaction`.
    drop(built);
    let bytes = Box::pin(builds.balance_transaction(&bytes)).await?;

    held.keep();
    Ok(bytes)
}
