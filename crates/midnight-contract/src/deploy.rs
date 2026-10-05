//! Contract deploy paths.
//!
//! - [`deploy_funded`] is the production path: takes a provider with a synced
//!   wallet, balances Dust fees, proves, and returns a [`DeployResult`].
//! - `wait_for_deployment` polls a provider until the deploy is visible.
//!
//! Prefer the high-level [`crate::Contract::deploy`] / [`crate::DeployBuilder`]
//! over calling these directly.

use std::time::Duration;

use midnight_provider::{Builds, TxInBlock};
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{ContractAddress, LedgerVersion, SpentInputs, WalletError};

use crate::ShieldedOffer;
use crate::address::format_address;
use crate::error::ContractError;
use crate::state::deserialize_state;

/// Result of deploying a contract (before or after submission).
pub struct DeployResult {
    /// The contract's on-chain address.
    pub address: ContractAddress,
    /// The proven transaction bytes, ready for
    /// [`midnight_provider::MidnightProvider::submit_reserved`].
    pub tx_bytes: Vec<u8>,
    /// The inputs the build reserved for this transaction, such as the Dust
    /// that pays its fee. Pass them to `submit_reserved` with `tx_bytes`, so
    /// that a rejection hands them back. Bytes that are never submitted keep
    /// them reserved until their TTL elapses.
    pub reserved: SpentInputs,
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

/// Wait until the contract that the deploy `in_block` applied shows on the
/// provider, and return its state.
///
/// Polls the provider every `poll_interval` until `remaining` passes, which is
/// the time left of the deploy deadline. Then it returns
/// [`ContractError::DeployTimeout`] with `in_block`, and logs the error of the
/// last poll when that poll failed. `timeout` is the full length of the
/// deadline, which the error reports.
pub(crate) async fn wait_for_deployment<P: midnight_provider::Provider>(
    provider: &P,
    address: &str,
    in_block: TxInBlock,
    remaining: Duration,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let mut last_error = None;
    let poll = async {
        loop {
            match provider.get_contract_state(address, None).await {
                Ok(Some(hex)) => return deserialize_state(&hex),
                Ok(None) => last_error = None,
                Err(e) => last_error = Some(e),
            }
            tokio::time::sleep(poll_interval).await;
        }
    };
    if let Ok(state) = tokio::time::timeout(remaining, poll).await {
        return state;
    }
    if let Some(error) = last_error {
        tracing::warn!(
            address,
            error = %error,
            "the indexer did not show the deployed contract before the deadline"
        );
    }
    Err(ContractError::DeployTimeout {
        address: address.to_owned(),
        transaction_hash: in_block.transaction_hash,
        timeout,
        in_block: Some(Box::new(in_block)),
    })
}
