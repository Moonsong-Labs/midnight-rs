//! A contract deploy on this generation.

use std::collections::BTreeMap;
use std::sync::Arc;

use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{CoinPublicKey, EncryptionPublicKey};

use super::call::{INTENT_SEGMENT, prove_without_fee};
use super::helpers;
use super::provider::Builds;
use super::shielded::KeyedOutput;
use super::types::PreparedInput;
use super::types::convert::{IntoLedger, IntoSdk};
use helpers::{
    BuildContext, BuildContractAction, BuildInput, BuildOutput, ContractDeploy, DefaultDB,
    FromContext, IntentInfo, OfferInfo, SplittableRng, StandardTransactionInfo, TokenInfo,
};

use crate::ShieldedOffer;
use crate::change::change_due;
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

    let tx_info = single_action_tx_info(builds, context.clone(), DeployAction { deploy });
    let tx_bytes = match shielded_offer {
        None => build_with_mock_fee_proofs(builds, tx_info).await?,
        Some(offer) => Box::pin(offer_transaction(builds, &context, tx_info, offer)).await?,
    };

    Ok(DeployResult { address, tx_bytes })
}

/// The transaction info for one intent, at [`INTENT_SEGMENT`], that holds
/// `action` alone.
pub(super) fn single_action_tx_info(
    builds: &Builds<'_>,
    context: Arc<BuildContext>,
    action: impl BuildContractAction<DefaultDB, BuildContext> + 'static,
) -> StandardTransactionInfo<DefaultDB, BuildContext> {
    let mut tx_info =
        StandardTransactionInfo::new_from_context(context, builds.proof_provider(), None);
    tx_info.add_intent(
        INTENT_SEGMENT,
        Box::new(IntentInfo {
            guaranteed_unshielded_offer: None,
            fallible_unshielded_offer: None,
            actions: vec![Box::new(action)],
        }),
    );
    tx_info
}

/// Build `tx_info` with an empty guaranteed offer, pay its fee, and serialize
/// the result. The wallet balances the fee against mock proofs.
///
/// Only a transaction with no user circuit can take this path: upstream
/// `MockProver::check` rejects a circuit that is not built in.
pub(super) async fn build_with_mock_fee_proofs(
    builds: &Builds<'_>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<Vec<u8>, ContractError> {
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.use_mock_proofs_for_fees(true);
    Ok(builds.build_funded(tx_info).await?.tx_bytes)
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
    let (prepared, held) = builds
        .prepare_shielded_inputs(context, &offer.coins, &mut tx_info.rng.split())
        .await?;
    held.keep_on_ok(pay_offer(
        builds,
        tx_info,
        offer,
        prepared,
        (change_cpk, change_epk),
    ))
    .await
}

/// Pay `offer` from the reserved `prepared` coins, then prove the deploy and
/// pay its fee.
///
/// Any surplus goes back to the wallet as change.
async fn pay_offer(
    builds: &Builds<'_>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
    prepared: Vec<PreparedInput>,
    (change_cpk, change_epk): (CoinPublicKey, EncryptionPublicKey),
) -> Result<Vec<u8>, ContractError> {
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

    let bytes = prove_without_fee(tx_info).await?;
    Ok(builds.balance_transaction(&bytes).await?)
}
