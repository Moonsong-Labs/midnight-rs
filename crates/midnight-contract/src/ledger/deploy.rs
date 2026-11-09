//! A contract deploy.

use std::sync::Arc;

use midnight_helpers::{
    BuildContext, BuildContractAction, BuildOutput, ContractDeploy, DefaultDB, FromContext,
    IntentInfo, OfferInfo, OutputInfo, ShieldedWallet, StandardTransactionInfo,
};
use midnight_provider::MidnightProvider;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::build::{self, Cover, Fill};
use midnight_types::convert::{IntoLedger, IntoSdk};
use midnight_types::{MidnightWallet, TxEnvelope};

use super::call::{INTENT_SEGMENT, prove_without_fee};

use crate::ShieldedOffer;
use crate::deploy::DeployResult;
use crate::error::ContractError;

/// Deploy `initial_state` with Dust fee payment from the provider's funded
/// wallet. See [`crate::deploy::deploy_funded`].
pub(crate) async fn deploy_funded<W: MidnightWallet>(
    provider: &MidnightProvider<W>,
    initial_state: ContractState<InMemoryDB>,
    shielded_offer: Option<&ShieldedOffer>,
) -> Result<DeployResult, ContractError> {
    let context = provider.chain_context().await?;
    let deploy = ContractDeploy::new(&mut rand::thread_rng(), initial_state);
    let address = deploy.address().into_sdk();

    struct DeployAction {
        deploy: ContractDeploy<DefaultDB>,
    }

    #[async_trait::async_trait]
    impl<C: midnight_helpers::BuilderContext<DefaultDB>> BuildContractAction<DefaultDB, C>
        for DeployAction
    {
        async fn build(
            &mut self,
            _rng: &mut midnight_helpers::StdRng,
            _context: Arc<C>,
            intent: &midnight_helpers::UnprovenIntent<DefaultDB>,
        ) -> midnight_helpers::UnprovenIntent<DefaultDB> {
            intent.add_deploy(self.deploy.clone())
        }
    }

    let tx_info = single_action_tx_info(provider, context, DeployAction { deploy });
    let transaction = match shielded_offer {
        None => build_with_mock_fee_proofs(provider, tx_info).await?,
        Some(offer) => Box::pin(offer_transaction(provider, tx_info, offer)).await?,
    };

    Ok(DeployResult {
        address,
        transaction,
    })
}

/// The transaction info for one intent, at [`INTENT_SEGMENT`], that holds
/// `action` alone.
pub(super) fn single_action_tx_info<W>(
    provider: &MidnightProvider<W>,
    context: Arc<BuildContext>,
    action: impl BuildContractAction<DefaultDB, BuildContext> + 'static,
) -> StandardTransactionInfo<DefaultDB, BuildContext> {
    let mut tx_info =
        StandardTransactionInfo::new_from_context(context, provider.proof_provider(), None);
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

/// Build `tx_info` with an empty guaranteed offer, let the wallet pay its
/// fee, prove the result, and let the wallet sign it. The wallet balances the
/// fee against mock proofs.
///
/// Only a transaction with no user circuit can take this path: upstream
/// `MockProver::check` rejects a circuit that is not built in.
pub(super) async fn build_with_mock_fee_proofs<W: MidnightWallet>(
    provider: &MidnightProvider<W>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> Result<TxEnvelope, ContractError> {
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.use_mock_proofs_for_fees(true);
    Ok(provider
        .fund_prove_sign(build::filled(Fill::Assembled(Box::new(tx_info))))
        .await?)
}

/// Add the payments of `offer` to the deploy in `tx_info`, prove it, and let
/// the wallet cover it.
///
/// The deploy proves with no fee. The wallet then covers it in a part of its
/// own, as it does for a circuit call: the coins of `offer` first, then coins
/// that it selects for what they leave, the change, and the Dust fee. One
/// reservation holds what the part spends, and the result carries it.
async fn offer_transaction<W: MidnightWallet>(
    provider: &MidnightProvider<W>,
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
) -> Result<TxEnvelope, ContractError> {
    let payments = offer.outputs.iter().map(|payment| {
        Box::new(OutputInfo {
            destination: ShieldedWallet::<DefaultDB>::from_pub_keys(
                payment.recipient.coin_public_key.into_ledger(),
                payment.recipient.enc_public_key.into_ledger(),
            ),
            token_type: payment.token_type.into_ledger(),
            value: payment.value,
        }) as Box<dyn BuildOutput<DefaultDB, BuildContext>>
    });
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: payments.collect(),
        transients: vec![],
    });

    let context = tx_info.context.clone();
    let external = prove_without_fee(tx_info).await?;
    let cover = Cover::Shortfalls {
        pins: offer.coins.iter().map(|coin| coin.nullifier).collect(),
        pay_fees: true,
    };
    let tx_info =
        StandardTransactionInfo::new_from_context(context, provider.proof_provider(), None);
    Ok(provider
        .fund_prove_sign(build::filled(Fill::Sealed {
            tx_info: Box::new(tx_info),
            external: Box::new(external),
            cover,
        }))
        .await?)
}
