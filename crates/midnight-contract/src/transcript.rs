//! The contract-call transcripts of a transaction that a block ran, and the
//! logs that each transcript emits.

use midnight_helpers::{ContractAction, DefaultDB, FinalizedTransaction, Sp, Transaction};
use midnight_onchain_runtime::state_value_ext::StateValueExt;
use midnight_onchain_runtime::vm::MAX_LOG_EMITTED;
use midnight_provider::TxOutcome;
use midnight_typed_state::{InMemoryDB, StateValue, tagged_deserialize};
use midnight_types::ContractAddress;
use midnight_types::convert::IntoSdk;

use crate::error::ContractError;
use crate::runtime::{LogEventType, Op, ResultModeVerify, VersionedLogItem};

/// The contract-call transcripts of a transaction, in the order in which the
/// ledger runs them: the guaranteed transcripts of every intent in ascending
/// segment id, then the fallible transcript of each fallible segment in
/// ascending id. A call split at a checkpoint gives two items. A deploy or a
/// maintenance update gives none.
///
/// # Errors
///
/// [`ContractError::Serialization`] when `bytes` is not a ledger 9
/// transaction. Bytes from [`BlockTransaction`] always decode.
///
/// [`BlockTransaction`]: midnight_provider::BlockTransaction
pub fn call_transcripts(
    bytes: &[u8],
    outcome: TxOutcome,
) -> Result<Vec<CallTranscript>, ContractError> {
    let transaction: FinalizedTransaction<DefaultDB> = tagged_deserialize(bytes)
        .map_err(|e| ContractError::Serialization(format!("deserialize transaction: {e}")))?;
    let Transaction::Standard(transaction) = transaction else {
        return Ok(Vec::new());
    };
    let fallible_segments = transaction.fallible_segments().len();
    let mut intents: Vec<_> = transaction
        .intents
        .iter()
        .map(|entry| (*entry.0, entry.1.clone()))
        .collect();
    intents.sort_unstable_by_key(|(segment, _)| *segment);

    let mut transcripts = Vec::new();
    for fallible in [false, true] {
        for (_, intent) in &intents {
            for action in intent.actions.iter_deref() {
                let ContractAction::Call(call) = action else {
                    continue;
                };
                let transcript = if fallible {
                    &call.fallible_transcript
                } else {
                    &call.guaranteed_transcript
                };
                let Some(transcript) = transcript else {
                    continue;
                };
                transcripts.push(CallTranscript {
                    address: call.address.into_sdk(),
                    entry_point: call.entry_point.0.clone(),
                    status: call_status(outcome, fallible, fallible_segments),
                    ops: Vec::from(&transcript.program),
                });
            }
        }
    }
    Ok(transcripts)
}

/// The status of a transcript under the transaction's `outcome`. `fallible`
/// tells a fallible transcript from a guaranteed one, and
/// `fallible_segments` counts the fallible segments of the transaction.
fn call_status(outcome: TxOutcome, fallible: bool, fallible_segments: usize) -> CallStatus {
    match (outcome, fallible) {
        (TxOutcome::Genesis, _) => CallStatus::Undecided,
        (TxOutcome::Applied, _) | (TxOutcome::PartialSuccess, false) => CallStatus::Applied,
        (TxOutcome::PartialSuccess, true) if fallible_segments == 1 => CallStatus::Failed,
        (TxOutcome::PartialSuccess, true) => CallStatus::Undecided,
    }
}

/// One transcript of a contract call in a block.
#[derive(Debug, Clone)]
pub struct CallTranscript {
    /// The address of the contract that the call names.
    pub address: ContractAddress,
    /// The ledger's entry point bytes: the circuit name for compactc output.
    pub entry_point: Vec<u8>,
    /// Whether the chain applied the transcript.
    pub status: CallStatus,
    /// The program as the transaction carries it, with the read result of
    /// each `popeq` and the literal of each `push`.
    pub ops: Vec<Op<ResultModeVerify, InMemoryDB>>,
}

impl CallTranscript {
    /// The logs that this transcript emits when it runs, in op order, as the
    /// ledger 9 VM decodes each `log` op. It takes each logged value from the
    /// `push` right before the `log`, which is exact for compactc output. It
    /// does not say whether the transcript ran: read `status` for that.
    ///
    /// # Errors
    ///
    /// [`UnreadableLog`] when a `log` does not directly follow a `push`, or
    /// when the `log` or its `push` is inside a `branch` or `jmp` window.
    /// Only a VM run against the contract state before the call gives such a
    /// log, so the read refuses it.
    pub fn logs(&self) -> Result<Vec<VersionedLogItem<InMemoryDB>>, UnreadableLog> {
        let mut logs = Vec::new();
        // A `branch` or a `jmp` at `i` can skip the ops `i + 1 ..= i + skip`.
        // An op is in a window when it comes before the end of an earlier one.
        let mut window_end = 0;
        let mut previous: Option<(&Op<ResultModeVerify, InMemoryDB>, bool)> = None;
        for (index, op) in self.ops.iter().enumerate() {
            let in_window = index < window_end;
            match op {
                Op::Log => match previous {
                    Some((Op::Push { value, .. }, false)) if !in_window => {
                        logs.extend(decode_event(value.clone().into()));
                    }
                    _ => return Err(UnreadableLog { op: index }),
                },
                Op::Branch { skip } | Op::Jmp { skip } => {
                    window_end = window_end.max((index + 1).saturating_add(*skip as usize));
                }
                _ => {}
            }
            previous = Some((op, in_window));
        }
        Ok(logs)
    }
}

/// The item that the ledger 9 VM logs for `value`, or `None` when the VM
/// drops it. It mirrors the VM's private `decode_event`.
fn decode_event(value: StateValue<InMemoryDB>) -> Option<VersionedLogItem<InMemoryDB>> {
    let item = event_envelope(&value).unwrap_or_else(|| VersionedLogItem {
        version: 0,
        event_type: LogEventType::Misc,
        data: value,
    });
    Sp::<_, InMemoryDB>::new(item.data.clone()).serialize_to_node_list_bounded(MAX_LOG_EMITTED)?;
    Some(item)
}

/// `value` as an event: an array of a `u32` version, an event tag and the
/// data. It mirrors the VM's private `try_decode_event`.
fn event_envelope(value: &StateValue<InMemoryDB>) -> Option<VersionedLogItem<InMemoryDB>> {
    let StateValue::Array(entries) = value else {
        return None;
    };
    if entries.len() != 3 {
        return None;
    }
    let version = u32::try_from(&*entries.get(0)?.as_cell_ref().ok()?.as_slice()).ok()?;
    let event_type =
        LogEventType::try_from(&*entries.get(1)?.as_cell_ref().ok()?.as_slice()).ok()?;
    Some(VersionedLogItem {
        version,
        event_type,
        data: entries.get(2)?.clone(),
    })
}

/// Whether the chain applied a transcript, as far as the node says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// Its segment applied, so its effects and its logs are on chain.
    Applied,
    /// Its segment failed, so the chain kept none of its effects or logs.
    Failed,
    /// The node does not say: a partial success with two or more fallible
    /// segments, or a transaction of block 0.
    Undecided,
}

/// A `log` op that the static read cannot decode exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("log op {op} is not a push then log pair outside every branch and jmp window")]
pub struct UnreadableLog {
    /// The index of the `log` op in [`ops`](CallTranscript::ops).
    pub op: usize,
}

#[cfg(test)]
mod tests {
    use midnight_onchain_runtime::cost_model::INITIAL_COST_MODEL;
    use midnight_onchain_runtime::result_mode::{GatherEvent, ResultModeGather};
    use midnight_onchain_runtime::vm::run_program;
    use midnight_typed_state::AlignedValue;
    use midnight_types::HashOutput;

    use super::*;
    use crate::runtime::bytes_aligned_value;

    /// It catches a reading of a partial success as "every fallible
    /// transcript failed", which is wrong with two fallible segments, and as
    /// "every transcript applied".
    #[test]
    fn a_transcript_status_follows_the_outcome_and_the_fallible_segments() {
        let cases = [
            (TxOutcome::Applied, true, 2, CallStatus::Applied),
            (TxOutcome::PartialSuccess, false, 2, CallStatus::Applied),
            (TxOutcome::PartialSuccess, true, 1, CallStatus::Failed),
            (TxOutcome::PartialSuccess, true, 2, CallStatus::Undecided),
            (TxOutcome::Genesis, false, 1, CallStatus::Undecided),
        ];
        for (outcome, fallible, fallible_segments, expected) in cases {
            assert_eq!(
                call_status(outcome, fallible, fallible_segments),
                expected,
                "{outcome:?}, fallible: {fallible}, {fallible_segments} fallible segments"
            );
        }
    }

    fn transcript(ops: Vec<Op<ResultModeVerify, InMemoryDB>>) -> CallTranscript {
        CallTranscript {
            address: ContractAddress(HashOutput([0; 32])),
            entry_point: b"record".to_vec(),
            status: CallStatus::Applied,
            ops,
        }
    }

    fn push(value: StateValue<InMemoryDB>) -> Op<ResultModeVerify, InMemoryDB> {
        Op::Push {
            storage: false,
            value: value.try_into_checked().expect("a canonical literal"),
        }
    }

    /// One cell of `data` under the alignment `bytes(width)`.
    fn cell(data: &[u8], width: usize) -> StateValue<InMemoryDB> {
        StateValue::from(bytes_aligned_value(data.to_vec(), width).expect("data fits the width"))
    }

    /// The value that compactc pushes for an event of `tag` whose data has
    /// `width` bytes: an array of the version 1, the tag and the data.
    fn event(tag: u8, width: usize) -> StateValue<InMemoryDB> {
        let data: Vec<u8> = (1..=width).map(|byte| byte as u8).collect();
        StateValue::Array(
            [
                StateValue::from(AlignedValue::from(1u32)),
                StateValue::from(AlignedValue::from(tag)),
                cell(&data, width),
            ]
            .into_iter()
            .collect(),
        )
    }

    /// The ledger 9 VM is the oracle: it runs `[push value, log]`, and
    /// `logs` must return the items that it logs. It catches a mirror that
    /// drifts from the VM's private `decode_event`. The cases are the envelope
    /// of each event tag, the version 0 form of a value that is not an event,
    /// and the drop above `MAX_LOG_EMITTED`.
    #[test]
    fn logs_decode_each_logged_value_as_the_vm_does() {
        // The data width of each event tag in compactc output.
        const WIDTHS: [usize; 11] = [32, 578, 81, 49, 145, 145, 80, 113, 0, 0, 288];
        let mut values: Vec<_> = (0..)
            .zip(WIDTHS)
            .map(|(tag, width)| event(tag, width))
            .collect();
        values.push(cell(&[0x2a; 40], 40));
        values.push(cell(&[0x2a; 2000], 2000));

        for value in values {
            let checked = value.try_into_checked().expect("a canonical literal");
            let program: [Op<ResultModeGather, InMemoryDB>; 2] = [
                Op::Push {
                    storage: false,
                    value: checked.clone(),
                },
                Op::Log,
            ];
            let logged: Vec<_> = run_program(&[], &program, None, &INITIAL_COST_MODEL)
                .expect("the VM runs a push then a log")
                .events
                .into_iter()
                .map(|event| match event {
                    GatherEvent::Log(item) => item,
                    GatherEvent::Read(read) => panic!("a push then a log reads nothing: {read:?}"),
                })
                .collect();

            let read = transcript(vec![
                Op::Push {
                    storage: false,
                    value: checked,
                },
                Op::Log,
            ])
            .logs();
            assert_eq!(read, Ok(logged), "for the logged value {value:?}");
        }
    }

    /// A `log` logs the top of the stack, so only a `push` right before it
    /// gives the value. It catches a read that takes the nearest earlier
    /// `push`.
    #[test]
    fn logs_refuse_a_log_after_a_dup() {
        let ops = vec![
            push(event(LogEventType::Misc as u8, 288)),
            Op::Dup { n: 0 },
            Op::Log,
        ];
        assert_eq!(transcript(ops).logs(), Err(UnreadableLog { op: 2 }));
    }

    /// Whether the VM runs an op inside a `branch` window depends on the
    /// state. It catches a read that ignores the window.
    #[test]
    fn logs_refuse_a_log_inside_a_branch_window() {
        let ops = vec![
            push(StateValue::from(AlignedValue::from(true))),
            Op::Branch { skip: 2 },
            push(event(LogEventType::Misc as u8, 288)),
            Op::Log,
        ];
        assert_eq!(transcript(ops).logs(), Err(UnreadableLog { op: 3 }));
    }
}
