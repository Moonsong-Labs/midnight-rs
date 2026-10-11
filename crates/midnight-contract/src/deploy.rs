//! Contract deploy paths.
//!
//! - [`deploy_funded`] is the production path: takes a provider with a synced
//!   wallet, balances Dust fees, proves, and returns a [`DeployResult`].
//! - `deployed_state` reads the deployed contract at the block that applied
//!   the deploy.
//!
//! Prefer the high-level [`crate::Contract::deploy`] / [`crate::DeployBuilder`]
//! over calling these directly.

use std::time::Duration;

use midnight_provider::{Builds, MidnightProvider, NodeBlockHash, TxInBlock};
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{ContractAddress, LedgerVersion, WalletError};

use crate::ShieldedOffer;
use crate::address::format_address;
use crate::error::ContractError;
use crate::state::fetch_state_from_node;

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

/// Read the state of the contract that the deploy `in_block` applied, at that
/// block.
///
/// `remaining` bounds the read: it is the time left of the deploy deadline.
/// Past it, the read gives [`ContractError::DeployTimeout`] with `in_block`.
/// `timeout` is the full length of the deadline, which the error reports. A
/// node that holds no contract at `address` in that block gives
/// [`ContractError::NotFound`].
pub(crate) async fn deployed_state(
    provider: &MidnightProvider,
    address: &str,
    in_block: TxInBlock,
    remaining: Duration,
    timeout: Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let at = NodeBlockHash::from(in_block.block_hash);
    let read = fetch_state_from_node(provider, address, Some(at));
    tokio::time::timeout(remaining, read)
        .await
        .unwrap_or_else(|_elapsed| {
            Err(ContractError::DeployTimeout {
                address: address.to_owned(),
                transaction_hash: in_block.transaction_hash,
                timeout,
                in_block: Some(Box::new(in_block)),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read gets only the time that the block wait left of the deploy
    /// deadline. The timeout keeps the inclusion that the deploy reached, so
    /// the caller connects and does not pay for a second deploy.
    #[tokio::test]
    async fn a_read_that_does_not_finish_times_out_at_the_deadline() {
        // A node that takes the connection and never answers it.
        let silent_node = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let node_url = format!("ws://{}", silent_node.local_addr().unwrap());
        let provider = MidnightProvider::new(&node_url, "http://test").unwrap();
        let applied = TxInBlock {
            block_hash: [1; 32],
            extrinsic_hash: [2; 32],
            transaction_hash: [3; 32].into(),
            verdict: midnight_provider::Verdict::Success,
        };

        let read = tokio::time::timeout(
            Duration::from_secs(5),
            deployed_state(
                &provider,
                "0200aa",
                applied,
                Duration::from_millis(50),
                Duration::from_secs(3600),
            ),
        )
        .await
        .expect("the read must stop at the deadline");

        let err = read.expect_err("a read that never finishes must time out");
        assert!(
            matches!(
                &err,
                ContractError::DeployTimeout { in_block: Some(in_block), .. }
                    if in_block.block_hash == applied.block_hash
            ),
            "got {err:?}"
        );
        assert!(
            err.to_string().contains("Contract::at"),
            "the timeout after inclusion must tell the caller to connect, got: {err}"
        );
    }
}
