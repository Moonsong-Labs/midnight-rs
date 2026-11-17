//! The logs that a contract emitted in one block, from the node.

use midnight_provider::{BlockTransaction, NodeApi, TransactionHash};
use midnight_typed_state::InMemoryDB;
use midnight_types::{BlockHash, ContractAddress};

use crate::contract::{AsMidnightProvider, Contract};
use crate::error::ContractError;
use crate::parse_contract_address;
use crate::runtime::VersionedLogItem;
use crate::transcript::{CallStatus, CallTranscript, call_transcripts};

/// A log that a contract emitted, with the transaction that emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    /// The ledger's hash of the transaction that holds the call.
    pub transaction_hash: TransactionHash,
    /// The ledger's entry point bytes of the call: the circuit name for
    /// compactc output.
    pub entry_point: Vec<u8>,
    /// The log, of the item type of
    /// [`CallOutcome::logs`](crate::CallOutcome::logs).
    /// [`Misc::decode_log_data`](crate::runtime::Misc::decode_log_data)
    /// decodes a `Misc` event.
    pub item: VersionedLogItem<InMemoryDB>,
}

impl<P> Contract<P> {
    /// Get the logs that this contract emitted in the block with
    /// `block_hash`, from the node.
    ///
    /// It reads the block with [`NodeApi::get_block_transactions`] and the
    /// calls of this contract with [`CallTranscript::logs`]. It gives the
    /// logs of the calls that the chain applied, in the order in which the
    /// chain ran them. It leaves out a call whose segment failed. So it gives
    /// every log that the chain kept in the block, or an error. It finds a
    /// call that a governance call wraps.
    ///
    /// It reads the block that `block_hash` names, not the
    /// [`at_block`](Self::at_block) pin. Pass a finalized block hash, because
    /// a reorg can replace a best block. The read needs the state at the
    /// block and at its parent, so a node with default pruning serves the
    /// last 255 finalized blocks.
    ///
    /// # Errors
    ///
    /// - [`ContractError::UnknownLogs`] when the node read cannot tell which
    ///   logs of this contract one transaction of the block kept.
    /// - [`ContractError::Provider`] when the node read of the block fails,
    ///   for example for a block that the node does not hold.
    /// - [`ContractError::InvalidContractAddress`] when the handle's address
    ///   does not parse.
    pub async fn logs_at(&self, block_hash: BlockHash) -> Result<Vec<Log>, ContractError>
    where
        P: AsMidnightProvider,
    {
        let address = parse_contract_address(self.address())?;
        let transactions = self
            .provider()
            .as_midnight_provider()
            .get_block_transactions(block_hash)
            .await?;
        let mut logs = Vec::new();
        for transaction in transactions {
            logs.extend(transaction_logs(&address, transaction)?);
        }
        Ok(logs)
    }
}

/// The logs of `address` that the chain kept from one transaction of a
/// block.
fn transaction_logs(
    address: &ContractAddress,
    transaction: BlockTransaction,
) -> Result<Vec<Log>, ContractError> {
    let transaction_hash = transaction.transaction_hash;
    let bytes = transaction
        .bytes
        .ok_or(ContractError::UnknownLogs { transaction_hash })?;
    let calls = call_transcripts(&bytes, transaction.outcome)?;
    contract_logs(address, transaction_hash, &calls)
}

/// The logs of the calls of `address` in `calls`, the transcripts of the
/// transaction `transaction_hash`.
fn contract_logs(
    address: &ContractAddress,
    transaction_hash: TransactionHash,
    calls: &[CallTranscript],
) -> Result<Vec<Log>, ContractError> {
    let mut logs = Vec::new();
    for call in calls.iter().filter(|call| call.address == *address) {
        let items = kept_logs(call).ok_or(ContractError::UnknownLogs { transaction_hash })?;
        logs.extend(items.into_iter().map(|item| Log {
            transaction_hash,
            entry_point: call.entry_point.clone(),
            item,
        }));
    }
    Ok(logs)
}

/// The logs that the chain kept from `call`, or `None` when the node read
/// cannot tell. A failed call kept none, and an undecided call that emits no
/// log needs no status.
fn kept_logs(call: &CallTranscript) -> Option<Vec<VersionedLogItem<InMemoryDB>>> {
    if call.status == CallStatus::Failed {
        return Some(Vec::new());
    }
    let logs = call.logs().ok()?;
    if call.status == CallStatus::Undecided && !logs.is_empty() {
        return None;
    }
    Some(logs)
}

#[cfg(test)]
mod tests {
    use midnight_provider::TxOutcome;
    use midnight_typed_state::{AlignedValue, StateValue};
    use midnight_types::HashOutput;

    use super::*;
    use crate::runtime::{Op, ResultModeVerify};

    fn call(status: CallStatus, ops: Vec<Op<ResultModeVerify, InMemoryDB>>) -> CallTranscript {
        call_of(1, status, ops)
    }

    fn call_of(
        address: u8,
        status: CallStatus,
        ops: Vec<Op<ResultModeVerify, InMemoryDB>>,
    ) -> CallTranscript {
        CallTranscript {
            address: ContractAddress(HashOutput([address; 32])),
            entry_point: b"record".to_vec(),
            status,
            ops,
        }
    }

    fn push_log() -> Vec<Op<ResultModeVerify, InMemoryDB>> {
        let value = StateValue::from(AlignedValue::from(7u32));
        vec![
            Op::Push {
                storage: false,
                value: value.try_into_checked().expect("a canonical literal"),
            },
            Op::Log,
        ]
    }

    /// The status matters only for a call that logs, and a failed call kept
    /// nothing. It catches a read that does one of these:
    /// - returns the logs of a failed call;
    /// - returns the logs of an undecided call as kept;
    /// - fails a block for an undecided call that logs nothing;
    /// - skips an unreadable log;
    /// - reads the logs of a call before it checks for `Failed`.
    #[test]
    fn a_call_gives_the_logs_that_the_chain_kept() {
        let unreadable = vec![Op::Dup { n: 0 }, Op::Log];
        let logged = call(CallStatus::Applied, push_log())
            .logs()
            .expect("a push then a log");
        let cases = [
            (CallStatus::Applied, push_log(), Some(logged)),
            (CallStatus::Failed, push_log(), Some(Vec::new())),
            (CallStatus::Failed, unreadable.clone(), Some(Vec::new())),
            (CallStatus::Undecided, Vec::new(), Some(Vec::new())),
            (CallStatus::Undecided, push_log(), None),
            (CallStatus::Applied, unreadable, None),
        ];
        for (status, ops, expected) in cases {
            let call = call(status, ops);
            assert_eq!(kept_logs(&call), expected, "{status:?}, {:?}", call.ops);
        }
    }

    /// It catches a read that returns the logs of another contract, and one
    /// that fails the block for an undecided call of another contract.
    #[test]
    fn a_transaction_gives_the_logs_of_the_contract_only() {
        let transaction_hash = TransactionHash::from([7; 32]);
        let calls = [
            call_of(1, CallStatus::Applied, push_log()),
            call_of(2, CallStatus::Applied, push_log()),
            call_of(2, CallStatus::Undecided, push_log()),
        ];
        let item = calls[0].logs().expect("a push then a log").remove(0);
        assert_eq!(
            contract_logs(
                &ContractAddress(HashOutput([1; 32])),
                transaction_hash,
                &calls
            )
            .expect("no undecided call of the contract"),
            [Log {
                transaction_hash,
                entry_point: b"record".to_vec(),
                item,
            }]
        );
    }

    /// It catches a read that skips a transaction that the block ran from
    /// storage, which can hold a call of the contract: a silent gap.
    #[test]
    fn a_transaction_with_no_copy_fails_the_read() {
        let transaction_hash = TransactionHash::from([7; 32]);
        let transaction = BlockTransaction {
            transaction_hash,
            outcome: TxOutcome::Applied,
            bytes: None,
        };
        assert!(matches!(
            transaction_logs(&ContractAddress(HashOutput([1; 32])), transaction),
            Err(ContractError::UnknownLogs { transaction_hash: hash }) if hash == transaction_hash
        ));
    }
}
