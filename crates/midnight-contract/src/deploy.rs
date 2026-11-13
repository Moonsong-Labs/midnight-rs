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

use midnight_provider::{MidnightProvider, TransactionReceipt};
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{ContractAddress, MidnightWallet, TxEnvelope};

use crate::ShieldedOffer;
use crate::address::format_contract_address;
use crate::error::ContractError;
use crate::state::fetch_state;

/// A deploy, built and proven, before its submit.
#[derive(Debug)]
pub struct DeployResult {
    /// The contract's on-chain address.
    pub address: ContractAddress,
    /// The signed deploy transaction, which carries the reservation of the
    /// inputs that the wallet spends: the fee Dust, and the coins of a
    /// shielded offer. Send it with [`MidnightProvider::send_tx_envelope`],
    /// which hands the inputs back when the node never gets the deploy or
    /// rejects it.
    pub transaction: TxEnvelope,
}

impl DeployResult {
    /// The contract address as a hex string.
    pub fn address_hex(&self) -> String {
        format_contract_address(&self.address)
    }
}

/// Deploy a contract with Dust fee payment from the provider's funded wallet.
///
/// Builds the deploy, runs the helpers' fee-balancing / proving pipeline,
/// and returns a [`DeployResult`] containing the contract address and the
/// proven transaction with the inputs its build reserved. A `shielded_offer`
/// rides in the same transaction, as
/// [`DeployBuilder::with_shielded_offer`](crate::DeployBuilder::with_shielded_offer)
/// describes.
///
/// # Errors
///
/// Returns [`ContractError::Provider`] when the wallet cannot spend a coin of
/// the offer or holds too little of a token that the offer pays, when the fee
/// balancing fails, or when proving fails.
pub async fn deploy_funded<W: MidnightWallet>(
    initial_state: &ContractState<InMemoryDB>,
    provider: &MidnightProvider<W>,
    shielded_offer: Option<ShieldedOffer>,
) -> Result<DeployResult, ContractError> {
    // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
    Box::pin(crate::ledger::deploy::deploy_funded(
        provider,
        recharged(initial_state),
        shielded_offer.as_ref(),
    ))
    .await
}

/// `state` with the charged keys of its data, as the ledger computes them for
/// a deploy.
///
/// The charged keys of a state that the interpreter or a simulator updated
/// come from each update in turn, and they can differ from the keys of the
/// data. The ledger refuses such a deploy with `IncorrectChargedState`.
fn recharged(state: &ContractState<InMemoryDB>) -> ContractState<InMemoryDB> {
    ContractState {
        balance: state.balance.clone(),
        ..ContractState::new(
            (*state.data.get()).clone(),
            state.operations.clone(),
            state.maintenance_authority.clone(),
        )
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
pub(crate) async fn deployed_state<W>(
    provider: &MidnightProvider<W>,
    address: &str,
    in_block: TransactionReceipt,
    remaining: Duration,
    timeout: Duration,
) -> Result<ContractState<InMemoryDB>, ContractError> {
    let at = in_block.block_hash;
    let mut last_error = None;
    let read = async {
        loop {
            match fetch_state(provider, address, Some(at)).await {
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
    use midnight_types::{BlockHash, ExtrinsicHash};

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
        let applied = TransactionReceipt {
            block_hash: BlockHash::repeat_byte(1),
            extrinsic_hash: ExtrinsicHash::repeat_byte(2),
            transaction_hash: [3; 32].into(),
        };

        for (node, node_url) in nodes {
            let provider = MidnightProvider::new(&node_url);
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
