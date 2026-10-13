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
use midnight_types::ContractAddress;

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
/// `shielded_offer` rides in the same transaction, as
/// [`DeployBuilder::with_shielded_offer`](crate::DeployBuilder::with_shielded_offer)
/// describes.
///
/// # Errors
///
/// Returns [`ContractError::ShieldedOfferShortfall`] when the offer's outputs
/// pay more of a token than its coins hold, and [`ContractError::Provider`]
/// when the wallet cannot spend a coin of the offer, the fee balancing fails,
/// or proving fails.
pub async fn deploy_funded(
    initial_state: &ContractState<InMemoryDB>,
    provider: &midnight_provider::MidnightProvider,
    shielded_offer: Option<ShieldedOffer>,
) -> Result<DeployResult, ContractError> {
    let offer = shielded_offer.as_ref();
    // Each arm is boxed so this frame holds one generation's future, not both.
    match provider.builds().await? {
        Builds::Ledger8(builds) => {
            Box::pin(crate::ledger_8::deploy::deploy_funded(
                &builds,
                initial_state,
                offer,
            ))
            .await
        }
        Builds::Ledger9(builds) => {
            Box::pin(crate::ledger_9::deploy::deploy_funded(
                &builds,
                initial_state,
                offer,
            ))
            .await
        }
    }
}

/// The pause before the next read of a deployed contract after a read fails.
/// The node client refuses a call while it reconnects, so a later read can
/// succeed.
const READ_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Read the state of the contract that the deploy `in_block` applied, at that
/// block.
///
/// A read that fails below the contract layer runs again until `remaining`
/// passes, which is the time left of the deploy deadline. Then it logs the last
/// failure and gives [`ContractError::DeployTimeout`] with `in_block`, so the
/// caller still learns that the chain applied the deploy. `timeout` is the full
/// length of the deadline, which the error reports. A node that holds no
/// contract at `address` in that block gives [`ContractError::NotFound`] at
/// once, and a state that does not decode gives [`ContractError::StateFetch`].
pub(crate) async fn deployed_state(
    provider: &MidnightProvider,
    address: &str,
    in_block: TxInBlock,
    remaining: Duration,
    timeout: Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let at = NodeBlockHash::from(in_block.block_hash);
    let mut last_error = None;
    let read = async {
        loop {
            match fetch_state_from_node(provider, address, Some(at)).await {
                Err(ContractError::Provider(error)) => last_error = Some(error),
                definitive => return definitive,
            }
            tokio::time::sleep(READ_RETRY_INTERVAL).await;
        }
    };
    if let Ok(state) = tokio::time::timeout(remaining, read).await {
        return state;
    }
    if let Some(error) = last_error {
        tracing::warn!(
            address,
            error = %error,
            "the read of the deployed contract failed until the deadline"
        );
    }
    Err(ContractError::DeployTimeout {
        address: address.to_owned(),
        transaction_hash: in_block.transaction_hash,
        timeout,
        in_block: Some(Box::new(in_block)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read gets only the time that the block wait left of the deploy
    /// deadline, whether it hangs or fails. The timeout keeps the inclusion
    /// that the deploy reached, so the caller connects and does not pay for a
    /// second deploy.
    #[tokio::test]
    async fn a_read_that_does_not_succeed_times_out_with_the_inclusion() {
        // Takes the connection and never answers it.
        let silent_node = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let nodes = [
            (
                "a silent node",
                format!("ws://{}", silent_node.local_addr().unwrap()),
            ),
            // Fails every read at once.
            ("a node URL that does not parse", "not a url".to_owned()),
        ];
        let applied = TxInBlock {
            block_hash: [1; 32],
            extrinsic_hash: [2; 32],
            transaction_hash: [3; 32].into(),
            verdict: midnight_provider::Verdict::Success,
        };

        for (node, node_url) in nodes {
            let provider = MidnightProvider::new(&node_url, "http://test");
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
            .unwrap_or_else(|_| panic!("the read from {node} must stop at the deadline"));

            let err = read.expect_err("a read that does not succeed must time out");
            assert!(
                matches!(
                    &err,
                    ContractError::DeployTimeout { in_block: Some(in_block), .. }
                        if in_block.block_hash == applied.block_hash
                ),
                "{node}: got {err:?}"
            );
            assert!(
                err.to_string().contains("Contract::at"),
                "{node}: the timeout after inclusion must tell the caller to connect, got: {err}"
            );
        }
    }
}
