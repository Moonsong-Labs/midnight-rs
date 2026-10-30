//! A contract deploy on this generation.

use std::collections::BTreeMap;
use std::sync::Arc;

use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{CoinPublicKey, EncryptionPublicKey, TransferResult};

use super::call::{INTENT_SEGMENT, prove_without_fee};
use super::helpers;
use super::provider::Builds;
use super::types::PreparedInput;
use super::types::convert::{IntoLedger, IntoSdk};
use helpers::{
    BuildContext, BuildContractAction, BuildInput, BuildOutput, ContractDeploy, DefaultDB,
    FromContext, IntentInfo, OfferInfo, OutputInfo, ShieldedWallet, SplittableRng,
    StandardTransactionInfo, TokenInfo,
};

use crate::ShieldedOffer;
use crate::change::{change_due, total_by_key};
use crate::deploy::DeployResult;
use crate::error::ContractError;
use crate::offer::refuse_shortfall;

/// Build and prove the deploy of `initial_state`. When `pay_fees`, the
/// provider's wallet also pays the Dust fee. See
/// [`crate::deploy::deploy_transaction`].
pub(crate) async fn deploy_transaction(
    builds: &Builds<'_>,
    initial_state: &ContractState<InMemoryDB>,
    shielded_offer: Option<&ShieldedOffer>,
    pay_fees: bool,
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
            intent: &helpers::UnprovenIntent<DefaultDB>,
        ) -> helpers::UnprovenIntent<DefaultDB> {
            intent.add_deploy(self.deploy.clone())
        }
    }

    let tx_info = single_action_tx_info(builds, context.clone(), DeployAction { deploy });
    let transaction = match (shielded_offer, pay_fees) {
        (None, true) => build_with_mock_fee_proofs(builds, tx_info).await?,
        (None, false) => prove_without_fee(tx_info).await?,
        (Some(offer), _) => {
            Box::pin(offer_transaction(
                builds, &context, tx_info, offer, pay_fees,
            ))
            .await?
        }
    };

    Ok(DeployResult {
        address,
        transaction,
    })
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

/// Build `tx_info` with an empty guaranteed offer, pay its fee, and prove the
/// result. The wallet balances the fee against mock proofs.
///
/// Only a transaction with no user circuit can take this path: upstream
/// `MockProver::check` rejects a circuit that is not built in.
pub(super) async fn build_with_mock_fee_proofs(
    builds: &Builds<'_>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<TransferResult, ContractError> {
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.use_mock_proofs_for_fees(true);
    Ok(builds.build_funded(tx_info).await?)
}

/// Add `offer` to the deploy in `tx_info`, prove it, and pay its fee when
/// `pay_fees`.
///
/// The wallet spends the offer's coins into `context` and reserves them. The
/// deploy then proves with no fee. When `pay_fees`, the wallet pays the Dust
/// fee in an intent of its own, as it does for a circuit call. `build_funded`
/// cannot pay it: it funds `context` again, and the wallet refuses a second
/// funding of one context.
///
/// The coins go back to the wallet when a step fails. Once the deploy is
/// proven, they stay reserved. The result names them beside the fee Dust, if
/// any, because `submit_reserved` hands back only what a result names.
async fn offer_transaction(
    builds: &Builds<'_>,
    context: &Arc<BuildContext>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
    pay_fees: bool,
) -> Result<TransferResult, ContractError> {
    let (change_cpk, change_epk) = builds.provider().shielded_public_keys().await?;

    // The spends need the funding view in the context. Reserving before it
    // would hide these coins from it.
    builds.add_funding(context).await?;
    let (prepared, held) = builds
        .prepare_shielded_inputs(context, &offer.coins, &mut tx_info.rng.split())
        .await?;
    let pinned = held.spent().clone();
    let mut transaction = held
        .keep_on_ok(pay_offer(
            builds,
            tx_info,
            offer,
            prepared,
            (change_cpk, change_epk),
            pay_fees,
        ))
        .await?;
    transaction.spent.merge(pinned);
    Ok(transaction)
}

/// Pay `offer` from the reserved `prepared` coins, then prove the deploy.
///
/// Any surplus goes back to the wallet as change. When `pay_fees`, the
/// wallet also pays the deploy's fee.
async fn pay_offer(
    builds: &Builds<'_>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
    prepared: Vec<PreparedInput>,
    (change_cpk, change_epk): (CoinPublicKey, EncryptionPublicKey),
    pay_fees: bool,
) -> Result<TransferResult, ContractError> {
    let held_value = total_by_key(
        prepared
            .iter()
            .map(|input| (input.token_type().into_sdk(), input.value())),
    )?;
    let paid_value = total_by_key(
        offer
            .outputs
            .iter()
            .map(|payment| (payment.token_type, payment.value)),
    )?;
    refuse_shortfall(&held_value, &paid_value)?;

    let payments = offer.outputs.iter().map(|payment| OutputInfo {
        destination: ShieldedWallet::<DefaultDB>::from_pub_keys(
            payment.recipient.coin_public_key.into_ledger(),
            payment.recipient.enc_public_key.into_ledger(),
        ),
        token_type: payment.token_type.into_ledger(),
        value: payment.value,
    });
    let change = change_due(&held_value, &paid_value, &BTreeMap::new())
        .into_iter()
        .map(|(token_type, value)| OutputInfo {
            destination: ShieldedWallet::<DefaultDB>::from_pub_keys(
                change_cpk.into_ledger(),
                change_epk.into_ledger(),
            ),
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

    let unpaid = prove_without_fee(tx_info).await?;
    if !pay_fees {
        return Ok(unpaid);
    }
    Ok(builds.balance_transaction(&unpaid.tx_bytes).await?)
}
