//! A contract deploy.

use std::sync::Arc;

use midnight_helpers::{
    BuildContext, BuildContractAction, BuildOutput, ContractDeploy, DefaultDB, IntentInfo,
    OfferInfo, OutputInfo, ShieldedWallet, StandardTransactionInfo,
};
use midnight_provider::MidnightProvider;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::build::ContractPart;
use midnight_types::convert::{IntoLedger, IntoSdk};
use midnight_types::{MidnightWallet, TransactionRequest};

use super::call::INTENT_SEGMENT;

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
    let tx_info = provider.transaction_info().await?;
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

    let tx_info = with_single_action(tx_info, DeployAction { deploy });
    let part = match shielded_offer {
        None => assembled(tx_info),
        Some(offer) => with_offer(tx_info, offer),
    };
    let transaction = Box::pin(provider.fill(TransactionRequest::contract(part))).await?;

    Ok(DeployResult {
        address,
        transaction,
    })
}

/// `tx_info` with one intent, at [`INTENT_SEGMENT`], that holds `action`
/// alone.
pub(super) fn with_single_action(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    action: impl BuildContractAction<DefaultDB, BuildContext> + 'static,
) -> StandardTransactionInfo<DefaultDB, BuildContext> {
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

/// `tx_info` with an empty guaranteed offer, for the wallet to pay its fee in
/// its own intents. The wallet balances the fee against mock proofs, and one
/// proof follows.
///
/// Only a transaction with no user circuit can take this path: upstream
/// `MockProver::check` rejects a circuit that is not built in.
pub(super) fn assembled(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
) -> ContractPart {
    tx_info.set_guaranteed_offer(OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    });
    tx_info.use_mock_proofs_for_fees(true);
    ContractPart::Assembled(Box::new(tx_info))
}

/// The deploy in `tx_info` with the payments of `offer`, for the wallet to
/// cover.
///
/// The deploy proves with no fee. The wallet then covers it in a part of its
/// own, as it does for a circuit call: the coins of `offer` first, then coins
/// that it selects for what they leave, the change, and the Dust fee. One
/// reservation holds what the part spends, and the result carries it.
fn with_offer(
    mut tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    offer: &ShieldedOffer,
) -> ContractPart {
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

    ContractPart::Covered {
        tx_info: Box::new(tx_info),
        pins: offer.coins.iter().map(|coin| coin.nullifier).collect(),
    }
}
