//! Contract deploy paths.
//!
//! - [`build_deploy_tx`] / [`deploy`] / [`deploy_with_provider`] are
//!   low-level deploy-only builders (no fee balancing).
//! - [`deploy_local`] runs the deploy against a `TestState` without a node.
//! - [`deploy_funded`] is the production path: takes a provider with a synced
//!   wallet, balances Dust fees, proves, and returns a [`DeployResult`].
//! - [`deploy_and_submit`] adds the submit step on top.
//! - [`wait_for_deployment`] polls a provider until the deploy is visible.
//!
//! Prefer the high-level [`crate::Contract::deploy`] / [`crate::DeployBuilder`]
//! over calling these directly.

use std::sync::Arc;

use midnight_base_crypto::time::Timestamp;
use midnight_bindgen::{ContractState, InMemoryDB};
use midnight_coin_structure::contract::ContractAddress;
use midnight_serialize::tagged_serialize;

use crate::address::format_address;
use crate::call::{
    DEFAULT_TTL, Sig, UnprovenTransaction, build_resolver, current_ttl, make_proof_provider,
};
use crate::error::ContractError;
use crate::state::deserialize_state;
use midnight_provider::PendingTx;

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

/// Build a deploy transaction for a contract with the given initial state.
///
/// Low-level API — prefer [`deploy`] or [`deploy_with_provider`].
#[doc(hidden)]
pub async fn build_deploy_tx(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<(ContractAddress, Vec<u8>), ContractError> {
    use midnight_ledger::structure::ContractDeploy;
    use midnight_ledger::structure::{Intent, Transaction};
    use rand::SeedableRng;

    let mut rng = rand::thread_rng();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    let ttl: Timestamp = current_ttl(DEFAULT_TTL);

    let intent: Intent<Sig, _, _, InMemoryDB> = Intent::new(
        &mut rng,
        None,
        None,
        Vec::new(),
        Vec::new(),
        vec![deploy],
        None,
        ttl,
    );

    let mut intents = midnight_storage::storage::HashMap::new();
    intents = intents.insert(0u16, intent);

    let tx: UnprovenTransaction = Transaction::from_intents(network_id, intents);

    let resolver = make_deploy_resolver()?;
    let prove_rng = rand::rngs::StdRng::from_entropy();
    let proven = midnight_ledger::test_utilities::tx_prove_bind(prove_rng, &tx, &resolver)
        .await
        .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    let mut bytes = Vec::new();
    tagged_serialize(&proven, &mut bytes)
        .map_err(|e| ContractError::Serialization(e.to_string()))?;

    Ok((address, bytes))
}

/// Deploy a contract into a local `TestState` (no node needed).
///
/// Bypasses balance checks; suitable for local testing. Returns the contract
/// address and the post-deploy `TestState`.
pub async fn deploy_local(
    initial_state: &ContractState<InMemoryDB>,
) -> Result<
    (
        ContractAddress,
        midnight_ledger::test_utilities::TestState<InMemoryDB>,
    ),
    ContractError,
> {
    use midnight_ledger::structure::{ContractDeploy, Transaction};
    use midnight_ledger::test_utilities::TestState;
    use midnight_ledger::verify::WellFormedStrictness;
    use rand::SeedableRng;

    let mut rng = rand::rngs::StdRng::from_entropy();

    let deploy = ContractDeploy::new(&mut rng, initial_state.clone());
    let address = deploy.address();

    let mut test_state: TestState<InMemoryDB> = TestState::new(&mut rng);

    let intents = midnight_ledger::test_utilities::test_intents(
        &mut rng,
        vec![],
        vec![],
        vec![deploy],
        test_state.time,
    );

    let tx: UnprovenTransaction = Transaction::from_intents("local-test", intents);
    let resolver = make_deploy_resolver()?;
    let proven = midnight_ledger::test_utilities::tx_prove_bind(rng.clone(), &tx, &resolver)
        .await
        .map_err(|e| ContractError::Construction(format!("proving failed: {e:?}")))?;

    let mut strictness = WellFormedStrictness::default();
    strictness.enforce_balancing = false;
    test_state
        .apply(&proven, strictness)
        .map_err(|e| ContractError::Construction(format!("apply failed: {e:?}")))?;

    Ok((address, test_state))
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
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
    shielded_offer: Option<midnight_helpers::OfferInfo<midnight_helpers::DefaultDB>>,
) -> Result<DeployResult, ContractError> {
    use midnight_helpers::{
        BuildContractAction, ContractDeploy as LhContractDeploy, DefaultDB, FromContext,
        IntentInfo, LedgerContext, OfferInfo, ProofProvider, StandardTrasactionInfo,
    };

    let wallet_seed = provider
        .seed()
        .await
        .map_err(|_| ContractError::Construction("provider has no wallet".into()))?;

    let context = provider
        .build_context()
        .await
        .map_err(|e| ContractError::Construction(format!("build context: {e}")))?;

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

    let resolver = build_resolver(keys_dir)?;
    context.update_resolver(resolver).await;

    let proof_provider: Arc<dyn ProofProvider<DefaultDB>> = make_proof_provider(prover);
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

/// Deploy a contract to a running node and submit the transaction in one step.
///
/// Convenience wrapper around [`deploy_funded`] + [`midnight_provider::MidnightProvider::submit`].
pub async fn deploy_and_submit(
    initial_state: &ContractState<InMemoryDB>,
    provider: &midnight_provider::MidnightProvider,
    keys_dir: &std::path::Path,
    prover: &crate::Prover,
) -> Result<(String, PendingTx), ContractError> {
    let result = deploy_funded(initial_state, provider, keys_dir, prover, None).await?;
    let pending = provider.submit(&result.tx_bytes).await?;
    Ok((result.address_hex(), pending))
}

/// Deploy a contract and return the address as a hex string.
///
/// Convenience wrapper around [`build_deploy_tx`] that also returns the
/// address in hex form.
pub async fn deploy(
    initial_state: &ContractState<InMemoryDB>,
    network_id: &str,
) -> Result<(String, Vec<u8>), ContractError> {
    let (address, tx_bytes) = build_deploy_tx(initial_state, network_id).await?;
    Ok((format_address(&address), tx_bytes))
}

/// Deploy a contract using a provider to look up the network ID.
pub async fn deploy_with_provider<P: midnight_provider::Provider>(
    provider: &P,
    initial_state: &ContractState<InMemoryDB>,
) -> Result<(String, Vec<u8>), ContractError> {
    let network_id = crate::call::fetch_network_id(provider).await?;
    deploy(initial_state, &network_id).await
}

/// Wait until a contract is deployed and visible via the provider.
///
/// Polls the provider every `poll_interval` until the contract state is found
/// or `timeout` is reached. Returns the contract state on success.
pub async fn wait_for_deployment<P: midnight_provider::Provider>(
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

/// Construct a `Resolver` for deploy transactions (no circuit proving keys
/// needed).
///
/// Deploy transactions contain no contract calls, so the external resolver
/// never fires — it always returns `Ok(None)`.
pub(crate) fn make_deploy_resolver()
-> Result<midnight_ledger::test_utilities::Resolver, ContractError> {
    use midnight_base_crypto::data_provider::{FetchMode, MidnightDataProvider, OutputMode};
    use midnight_ledger::dust::{DUST_EXPECTED_FILES, DustResolver};
    use midnight_ledger::prove::Resolver;
    use midnight_ledger::test_utilities::PUBLIC_PARAMS;

    let dust_resolver = DustResolver(
        MidnightDataProvider::new(
            FetchMode::OnDemand,
            OutputMode::Log,
            DUST_EXPECTED_FILES.to_owned(),
        )
        .map_err(|e| ContractError::Construction(format!("dust resolver: {e}")))?,
    );

    Ok(Resolver::new(
        PUBLIC_PARAMS.clone(),
        dust_resolver,
        Box::new(|_| Box::pin(std::future::ready(Ok(None)))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_bindgen::{ContractMaintenanceAuthority, StateValue, StorageHashMap};

    fn make_counter_state(round: u64) -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![StateValue::from(round)].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    #[tokio::test]
    async fn deploy_returns_hex_address() {
        if std::env::var("MIDNIGHT_LEDGER_TEST_STATIC_DIR").is_err() {
            eprintln!("skipping: MIDNIGHT_LEDGER_TEST_STATIC_DIR not set");
            return;
        }
        let state = make_counter_state(0);
        let (addr_hex, tx_bytes) = deploy(&state, "test").await.unwrap();
        assert_eq!(addr_hex.len(), 64);
        assert!(!tx_bytes.is_empty());
    }
}
