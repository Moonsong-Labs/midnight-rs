//! The decode behind
//! [`NodeApi::get_block_transactions`](crate::NodeApi::get_block_transactions):
//! the copies of `Midnight.send_mn_transaction` in a block body, the outcome
//! events in the block's `System.Events`, and the pairing of the two.

use subxt::client::OnlineClientAtBlockImpl;
use subxt::dynamic::Value;
use subxt::error::EventsError;
use subxt::events::Events;
use subxt::ext::scale_value::{ValueDef, Variant};
use subxt::extrinsics::Extrinsics;
use subxt::{Metadata, SubstrateConfig};

use crate::pipeline::deserialize_transaction;
use crate::{BlockTransaction, ProviderError, TransactionHash, TxOutcome};

const MIDNIGHT: &str = "Midnight";
const SEND_MN_TRANSACTION: &str = "send_mn_transaction";
const TX_APPLIED: &str = "TxApplied";
const TX_PARTIAL_SUCCESS: &str = "TxPartialSuccess";

/// The storage key of `System.Events`.
pub(crate) fn system_events_key() -> [u8; 32] {
    subxt::ext::frame_decode::storage::encode_storage_key_prefix("System", "Events")
}

/// Refuse `metadata` that has no `Midnight.send_mn_transaction` call, or no
/// `Midnight.TxApplied` or `Midnight.TxPartialSuccess` event. The read finds
/// them by name, so a rename would read as a block with fewer transactions.
pub(crate) fn check_names(metadata: &Metadata) -> Result<(), ProviderError> {
    let pallet = metadata.pallet_by_name(MIDNIGHT);
    let has_call =
        pallet.is_some_and(|pallet| pallet.call_variant_by_name(SEND_MN_TRANSACTION).is_some());
    let events = pallet.and_then(|pallet| pallet.event_variants());
    let has_event =
        |name: &str| events.is_some_and(|events| events.iter().any(|event| event.name == name));
    if has_call && has_event(TX_APPLIED) && has_event(TX_PARTIAL_SUCCESS) {
        Ok(())
    } else {
        Err(ProviderError::Rpc(format!(
            "the runtime metadata has no {MIDNIGHT}.{SEND_MN_TRANSACTION} call, or no \
             {MIDNIGHT}.{TX_APPLIED} or {MIDNIGHT}.{TX_PARTIAL_SUCCESS} event"
        )))
    }
}

/// A Midnight transaction in a block body, with the ledger's
/// `transaction_hash()` when it decodes as a ledger 9 transaction.
type BodyCopy = (Option<TransactionHash>, Vec<u8>);

/// The stack of the thread that decodes a block body. The runtime admits a
/// call nested 255 levels deep, and the decode of such a call overflows a
/// 2 MiB stack in a debug build.
const BODY_DECODE_STACK: usize = 8 << 20;

/// The Midnight transactions in a block body, in body order.
pub(crate) fn body_copies(
    extrinsics: &Extrinsics<'_, SubstrateConfig, OnlineClientAtBlockImpl<SubstrateConfig>>,
) -> Result<Vec<BodyCopy>, ProviderError> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(BODY_DECODE_STACK)
            .spawn_scoped(scope, || body_copies_on_this_stack(extrinsics))
            .map_err(|e| ProviderError::Rpc(format!("starting the block body decode: {e}")))?
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn body_copies_on_this_stack(
    extrinsics: &Extrinsics<'_, SubstrateConfig, OnlineClientAtBlockImpl<SubstrateConfig>>,
) -> Result<Vec<BodyCopy>, ProviderError> {
    let mut copies = Vec::new();
    for extrinsic in extrinsics.iter() {
        let call = extrinsic
            .map_err(|e| ProviderError::Rpc(format!("decoding the block body: {e}")))?
            .decode_call_data_as::<Value<()>>()
            .map_err(|e| ProviderError::Rpc(format!("decoding the block body: {e}")))?;
        collect_midnight_transactions(&call, &mut copies)?;
    }
    Ok(copies
        .into_iter()
        .map(|bytes| {
            let hash = deserialize_transaction(&bytes)
                .ok()
                .map(|transaction| TransactionHash::from(transaction.transaction_hash().0.0));
            (hash, bytes)
        })
        .collect())
}

/// Push the argument of each `Midnight.send_mn_transaction` in `call` to
/// `copies`, at any depth of the call.
fn collect_midnight_transactions(
    call: &Value<()>,
    copies: &mut Vec<Vec<u8>>,
) -> Result<(), ProviderError> {
    let fields = match &call.value {
        ValueDef::Variant(variant) => {
            if let Some(bytes) = send_mn_transaction_argument(variant)? {
                copies.push(bytes);
                return Ok(());
            }
            &variant.values
        }
        ValueDef::Composite(composite) => composite,
        _ => return Ok(()),
    };
    fields
        .values()
        .try_for_each(|field| collect_midnight_transactions(field, copies))
}

/// The bytes that `pallet` passes when it is the call
/// `Midnight.send_mn_transaction`, or `None` for any other value.
fn send_mn_transaction_argument(pallet: &Variant<()>) -> Result<Option<Vec<u8>>, ProviderError> {
    if pallet.name != MIDNIGHT {
        return Ok(None);
    }
    let Some(ValueDef::Variant(call)) = pallet.values.values().next().map(|call| &call.value)
    else {
        return Ok(None);
    };
    if call.name != SEND_MN_TRANSACTION {
        return Ok(None);
    }
    let mut arguments = call.values.values();
    match (arguments.next().and_then(byte_sequence), arguments.next()) {
        (Some(bytes), None) => Ok(Some(bytes)),
        _ => Err(ProviderError::Rpc(format!(
            "a {MIDNIGHT}.{SEND_MN_TRANSACTION} call does not take one byte sequence"
        ))),
    }
}

/// The bytes of a decoded `Vec<u8>`.
fn byte_sequence(value: &Value<()>) -> Option<Vec<u8>> {
    let ValueDef::Composite(bytes) = &value.value else {
        return None;
    };
    bytes
        .values()
        .map(|byte| byte.as_u128().and_then(|byte| u8::try_from(byte).ok()))
        .collect()
}

/// The `Midnight.TxApplied` and `Midnight.TxPartialSuccess` events of
/// `events`, in event order, each with the transaction hash it names.
pub(crate) fn outcome_events(
    events: &Events<SubstrateConfig>,
) -> Result<Vec<(TransactionHash, TxOutcome)>, ProviderError> {
    let decode_error = |e: EventsError| ProviderError::Rpc(format!("decoding System.Events: {e}"));
    let mut outcomes = Vec::new();
    for event in events.iter() {
        let event = event.map_err(decode_error)?;
        let outcome = match (event.pallet_name(), event.event_name()) {
            (MIDNIGHT, TX_APPLIED) => TxOutcome::Applied,
            (MIDNIGHT, TX_PARTIAL_SUCCESS) => TxOutcome::PartialSuccess,
            _ => continue,
        };
        let (tx_hash,) = event
            .decode_fields_unchecked_as::<([u8; 32],)>()
            .map_err(decode_error)?;
        outcomes.push((TransactionHash::from(tx_hash), outcome));
    }
    Ok(outcomes)
}

/// The transactions that a block ran, one for each of `outcomes` and in that
/// order. Each has the bytes of its copy in `copies` when the body holds one.
///
/// `outcomes` is `None` for block 0, which has no events. Each copy then
/// comes in body order, as [`TxOutcome::Genesis`].
///
/// A copy with no hash does not decode as a ledger 9 transaction. It is an
/// error in block 0, and when an outcome has no copy, because the block can
/// have run it. A governance call can carry such a copy and not run it, so
/// the copy alone is not an error.
pub(crate) fn pair_with_outcomes(
    copies: Vec<BodyCopy>,
    outcomes: Option<Vec<(TransactionHash, TxOutcome)>>,
) -> Result<Vec<BlockTransaction>, ProviderError> {
    let undecodable = copies.iter().any(|(hash, _)| hash.is_none());
    let copies: Vec<_> = copies
        .into_iter()
        .filter_map(|(hash, bytes)| Some((hash?, bytes)))
        .collect();
    let genesis = outcomes.is_none();
    let transactions: Vec<_> = match outcomes {
        None => copies
            .into_iter()
            .map(|(transaction_hash, bytes)| BlockTransaction {
                transaction_hash,
                outcome: TxOutcome::Genesis,
                bytes: Some(bytes),
            })
            .collect(),
        Some(outcomes) => outcomes
            .into_iter()
            .map(|(transaction_hash, outcome)| BlockTransaction {
                transaction_hash,
                outcome,
                bytes: copies
                    .iter()
                    .find(|(hash, _)| *hash == transaction_hash)
                    .map(|(_, bytes)| bytes.clone()),
            })
            .collect(),
    };
    if undecodable
        && (genesis
            || transactions
                .iter()
                .any(|transaction| transaction.bytes.is_none()))
    {
        return Err(ProviderError::Rpc(format!(
            "a {MIDNIGHT} transaction of the block does not decode as a ledger 9 transaction"
        )));
    }
    Ok(transactions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> TransactionHash {
        TransactionHash::from([byte; 32])
    }

    fn ran(byte: u8, outcome: TxOutcome, bytes: Option<&[u8]>) -> BlockTransaction {
        BlockTransaction {
            transaction_hash: hash(byte),
            outcome,
            bytes: bytes.map(<[u8]>::to_vec),
        }
    }

    /// The events decide what a block ran and in which order. It catches a
    /// pairing that skips an outcome whose transaction the body does not hold
    /// (the indexer's behaviour). It also catches a pairing that reports a
    /// copy with no outcome, and one that keeps the body order.
    #[test]
    fn a_block_ran_one_transaction_for_each_outcome_event_in_event_order() {
        let copies = vec![
            (Some(hash(1)), vec![1]),
            (Some(hash(2)), vec![2]),
            (Some(hash(3)), vec![3]),
        ];
        let outcomes = vec![
            (hash(2), TxOutcome::Applied),
            (hash(4), TxOutcome::PartialSuccess),
            (hash(1), TxOutcome::PartialSuccess),
        ];

        assert_eq!(
            pair_with_outcomes(copies, Some(outcomes)).expect("every copy decodes"),
            [
                ran(2, TxOutcome::Applied, Some(&[2])),
                ran(4, TxOutcome::PartialSuccess, None),
                ran(1, TxOutcome::PartialSuccess, Some(&[1])),
            ]
        );
    }

    /// Block 0 has no events, so a pairing by outcome drops its
    /// transactions.
    #[test]
    fn block_0_ran_every_copy_in_body_order() {
        let copies = vec![(Some(hash(2)), vec![2]), (Some(hash(1)), vec![1])];

        assert_eq!(
            pair_with_outcomes(copies, None).expect("every copy decodes"),
            [
                ran(2, TxOutcome::Genesis, Some(&[2])),
                ran(1, TxOutcome::Genesis, Some(&[1])),
            ]
        );
    }

    /// A copy that does not decode as a ledger 9 transaction can be a
    /// transaction that the block ran. A block before a ledger fork holds
    /// such copies. It catches a pairing that reports such a transaction as
    /// a run from storage, or drops it from block 0. It also catches a
    /// pairing that refuses a copy that a governance call carries and does
    /// not run.
    #[test]
    fn a_copy_that_does_not_decode_fails_the_read_only_when_the_block_can_have_run_it() {
        let copies = vec![(Some(hash(1)), vec![1]), (None, vec![9])];

        assert_eq!(
            pair_with_outcomes(copies.clone(), Some(vec![(hash(1), TxOutcome::Applied)]))
                .expect("every outcome has its copy"),
            [ran(1, TxOutcome::Applied, Some(&[1]))]
        );
        let outcome_with_no_copy =
            vec![(hash(1), TxOutcome::Applied), (hash(2), TxOutcome::Applied)];
        assert!(matches!(
            pair_with_outcomes(copies.clone(), Some(outcome_with_no_copy)),
            Err(ProviderError::Rpc(_))
        ));
        assert!(matches!(
            pair_with_outcomes(copies, None),
            Err(ProviderError::Rpc(_))
        ));
    }
}
