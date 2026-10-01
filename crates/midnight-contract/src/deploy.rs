//! Contract deploy paths.
//!
//! - [`deploy_funded`] is the production path: takes a provider with a synced
//!   wallet, balances Dust fees, proves, and returns a [`DeployResult`].
//! - `wait_for_deployment` polls a provider until the deploy is visible.
//!
//! Prefer the high-level [`crate::Contract::deploy`] / [`crate::DeployBuilder`]
//! over calling these directly.

use midnight_provider::Builds;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{ContractAddress, LedgerVersion, WalletError};

use crate::ShieldedOffer;
use crate::address::format_address;
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
/// Builds the deploy for the ledger generation the wallet's state is in,
/// runs the helpers' fee-balancing / proving pipeline, and returns a
/// [`DeployResult`] containing the contract address and proven TX bytes. A
/// `shielded_offer` must be built for that same generation.
pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    provider: &midnight_provider::MidnightProvider,
    shielded_offer: Option<ShieldedOffer>,
) -> Result<DeployResult, ContractError> {
    let mismatch = |expected, found| {
        ContractError::from(midnight_provider::ProviderError::from(
            WalletError::LedgerMismatch { expected, found },
        ))
    };
    // Each arm is boxed so this frame holds one generation's future, not both.
    match provider.builds().await? {
        Builds::Ledger8(builds) => {
            let offer = match shielded_offer {
                None => None,
                Some(ShieldedOffer::Ledger8(offer)) => Some(offer),
                Some(ShieldedOffer::Ledger9(_)) => {
                    return Err(mismatch(LedgerVersion::V8, LedgerVersion::V9));
                }
            };
            Box::pin(crate::ledger_8::deploy::deploy_funded(
                &builds,
                initial_state,
                offer,
            ))
            .await
        }
        Builds::Ledger9(builds) => {
            let offer = match shielded_offer {
                None => None,
                Some(ShieldedOffer::Ledger9(offer)) => Some(offer),
                Some(ShieldedOffer::Ledger8(_)) => {
                    return Err(mismatch(LedgerVersion::V9, LedgerVersion::V8));
                }
            };
            Box::pin(crate::ledger_9::deploy::deploy_funded(
                &builds,
                initial_state,
                offer,
            ))
            .await
        }
    }
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
