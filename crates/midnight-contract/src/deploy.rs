//! Contract deploy paths.
//!
//! - [`deploy_funded`] is the production path: takes a provider with a synced
//!   wallet, balances Dust fees, proves, and returns a [`DeployResult`].
//! - [`wait_for_deployment`] polls a provider until the deploy is visible.
//!
//! Prefer the high-level [`crate::Contract::deploy`] / [`crate::DeployBuilder`]
//! over calling these directly.

use std::sync::Arc;

use midnight_bindgen_runtime::{ContractState, InMemoryDB};
use midnight_coin_structure::contract::ContractAddress;
use midnight_serialize::tagged_serialize;

use crate::address::format_address;
use crate::call::build_resolver;
use crate::error::ContractError;
use crate::state::deserialize_state;

/// Result of deploying a contract (before or after submission).
pub struct DeployResult {
    /// The contract's on-chain address.
    pub address: ContractAddress,
    /// The proven transaction bytes, ready for [`midnight_provider::MidnightProvider::submit`].
    pub tx_bytes: Vec<u8>,
}

impl DeployResult {
    /// The contract address as a hex string.
    pub fn address_hex(&self) -> String {
        format_address(&self.address)
    }
}

/// Deploy a contract with Dust fee payment from the provider's funded wallet.
///
/// Builds a funded transaction by asking the provider for a fresh
/// [`midnight_helpers::LedgerContext`] (resyncs the wallet, then constructs
/// the context from the wallet's local state) and runs the helpers'
/// fee-balancing / proving pipeline.
///
/// Returns a [`DeployResult`] containing the contract address and proven TX
/// bytes.
pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    provider: &midnight_provider::MidnightProvider,
    zk_config: Arc<dyn crate::zk_config::ZkConfigProvider>,
    shielded_offer: Option<midnight_helpers::OfferInfo<midnight_helpers::DefaultDB>>,
) -> Result<DeployResult, ContractError> {
    use midnight_helpers::{
        BuildContractAction, ContractDeploy as LhContractDeploy, DefaultDB, FromContext,
        IntentInfo, LedgerContext, OfferInfo, ProofProvider, StandardTrasactionInfo,
    };

    let wallet_seed = provider.seed().await?;

    let context = provider.build_context().await?;

    let mut state_bytes = Vec::new();
    tagged_serialize(initial_state, &mut state_bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;
    let state_for_deploy: midnight_helpers::ContractState<DefaultDB> =
        midnight_helpers::deserialize(&mut state_bytes.as_slice())
            .map_err(|e| ContractError::Construction(format!("state conversion: {e}")))?;

    let deploy = LhContractDeploy::new(&mut rand::thread_rng(), state_for_deploy);
    let address_raw = deploy.address();
    let address = ContractAddress(midnight_base_crypto::hash::HashOutput(address_raw.0.0));

    struct DeployAction<D: midnight_helpers::DB + Clone> {
        deploy: LhContractDeploy<D>,
    }

    #[async_trait::async_trait]
    impl<D: midnight_helpers::DB + Clone> BuildContractAction<D> for DeployAction<D> {
        async fn build(
            &mut self,
            _rng: &mut midnight_helpers::StdRng,
            _context: Arc<LedgerContext<D>>,
            intent: &midnight_helpers::Intent<
                midnight_helpers::Signature,
                midnight_helpers::ProofPreimageMarker,
                midnight_helpers::PedersenRandomness,
                D,
            >,
        ) -> midnight_helpers::Intent<
            midnight_helpers::Signature,
            midnight_helpers::ProofPreimageMarker,
            midnight_helpers::PedersenRandomness,
            D,
        > {
            intent.add_deploy(self.deploy.clone())
        }
    }

    let deploy_action = DeployAction { deploy };

    let intent_info: IntentInfo<DefaultDB> = IntentInfo {
        guaranteed_unshielded_offer: None,
        fallible_unshielded_offer: None,
        actions: vec![Box::new(deploy_action)],
    };

    let resolver = build_resolver(zk_config)?;
    context.update_resolver(resolver).await;

    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = provider.proof_provider();
    let reserved_at = context.latest_block_context().tblock;
    let mut tx_info = StandardTrasactionInfo::new_from_context(context, proof_provider, None);
    tx_info.add_intent(1, Box::new(intent_info));
    tx_info.set_guaranteed_offer(shielded_offer.unwrap_or_else(|| OfferInfo {
        inputs: vec![],
        outputs: vec![],
        transients: vec![],
    }));
    tx_info.set_funding_seeds(vec![wallet_seed]);
    tx_info.use_mock_proofs_for_fees(true);

    let built = midnight_wallet::transfer::build_no_validate(tx_info)
        .await
        .map_err(|e| ContractError::Construction(format!("prove/balance failed: {e}")))?;

    // Reserve the dust spends used to fund this transaction on the
    // provider's wallet so a follow-up build before the indexer surfaces
    // the spend events does not re-select the same UTXOs. Pending entries
    // are cleared when matching events arrive or when their TTL elapses.
    if let Ok(mut wallet) = provider.wallet_mut().await {
        wallet.reserve_pending(built.dust_batches, Vec::new(), reserved_at);
    }

    let mut bytes = Vec::new();
    midnight_helpers::midnight_serialize::tagged_serialize(&built.finalized, &mut bytes)
        .map_err(|e| ContractError::Serialization(format!("{e}")))?;

    Ok(DeployResult {
        address,
        tx_bytes: bytes,
    })
}

/// Wait until a contract is deployed and visible via the provider.
///
/// Polls the provider every `poll_interval` until the contract state is found
/// or `timeout` is reached. Returns the contract state on success.
pub(crate) async fn wait_for_deployment<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let start = std::time::Instant::now();
    loop {
        match provider.get_contract_state(address, None).await {
            Ok(Some(hex)) => return deserialize_state(&hex),
            Ok(None) => {}
            Err(e) => {
                if start.elapsed() >= timeout {
                    return Err(ContractError::StateFetch(format!(
                        "timeout waiting for contract {address}: {e}"
                    )));
                }
            }
        }
        if start.elapsed() >= timeout {
            return Err(ContractError::StateFetch(format!(
                "timeout after {:.0}s waiting for contract {address}",
                timeout.as_secs_f64()
            )));
        }
        tokio::time::sleep(poll_interval).await;
    }
}
