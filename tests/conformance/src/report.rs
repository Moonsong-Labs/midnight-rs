//! Build the canonical per-step report from an interpreter `ExecutionResult`.
//!
//! Field-for-field mirror of what `ts-driver/driver.mjs` extracts from the TS
//! runtime's `CircuitResults`/`ProofData`. Every field must be produced by
//! both executors; anything one side cannot compute canonically stays out of
//! the report.

use midnight_bindgen_runtime::{AlignedValue, ContractState, InMemoryDB, StateValue};
use midnight_contract::runtime::ExecutionResult;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use serde_json::{Value as Json, json};

use crate::state_json::{aligned_value_to_json, state_value_to_json};

/// The public transcript as canonical JSON: gather-mode ops with each
/// `popeq` placeholder substituted by the corresponding read result, the
/// exact translation `call.rs` applies before partitioning transcripts.
pub fn public_transcript_json(result: &ExecutionResult) -> Json {
    let mut read_iter = result.reads.iter();
    let verify_ops: Vec<midnight_onchain_runtime::ops::Op<ResultModeVerify, InMemoryDB>> = result
        .gather_ops
        .iter()
        .map(|op| {
            op.clone().translate(|()| {
                read_iter
                    .next()
                    .cloned()
                    .unwrap_or_else(|| AlignedValue::from(()))
            })
        })
        .collect();
    Json::Array(verify_ops.iter().map(op_to_json).collect())
}

/// Canonical JSON for a single transcript op, matching the TS `Op<R>` union:
/// payload-free ops are plain strings, payload ops single-key objects. Byte
/// content routes through the hex encoders in `state_json`.
pub fn op_to_json(op: &midnight_onchain_runtime::ops::Op<ResultModeVerify, InMemoryDB>) -> Json {
    use midnight_onchain_runtime::ops::{Key, Op};
    match op {
        Op::Noop { n } => json!({ "noop": { "n": n } }),
        Op::Lt => json!("lt"),
        Op::Eq => json!("eq"),
        Op::Type => json!("type"),
        Op::Size => json!("size"),
        Op::New => json!("new"),
        Op::And => json!("and"),
        Op::Or => json!("or"),
        Op::Neg => json!("neg"),
        Op::Log => json!("log"),
        Op::Root => json!("root"),
        Op::Pop => json!("pop"),
        Op::Popeq { cached, result } => json!({
            "popeq": { "cached": cached, "result": aligned_value_to_json(result) },
        }),
        Op::Addi { immediate } => json!({ "addi": { "immediate": immediate } }),
        Op::Subi { immediate } => json!({ "subi": { "immediate": immediate } }),
        Op::Push { storage, value } => json!({
            "push": { "storage": storage, "value": state_value_to_json(value) },
        }),
        Op::Branch { skip } => json!({ "branch": { "skip": skip } }),
        Op::Jmp { skip } => json!({ "jmp": { "skip": skip } }),
        Op::Add => json!("add"),
        Op::Sub => json!("sub"),
        Op::Concat { cached, n } => json!({ "concat": { "cached": cached, "n": n } }),
        Op::Member => json!("member"),
        Op::Rem { cached } => json!({ "rem": { "cached": cached } }),
        Op::Dup { n } => json!({ "dup": { "n": n } }),
        Op::Swap { n } => json!({ "swap": { "n": n } }),
        Op::Idx {
            cached,
            push_path,
            path,
        } => json!({
            "idx": {
                "cached": cached,
                "pushPath": push_path,
                "path": path
                    .iter()
                    .map(|key| match &*key {
                        Key::Value(av) => {
                            json!({ "tag": "value", "value": aligned_value_to_json(av) })
                        }
                        Key::Stack => json!({ "tag": "stack" }),
                    })
                    .collect::<Vec<_>>(),
            },
        }),
        Op::Ins { cached, n } => json!({ "ins": { "cached": cached, "n": n } }),
        Op::Ckpt => json!("ckpt"),
        other => panic!("unhandled transcript op: {other:?}"),
    }
}

/// Concatenated circuit input, through the same typed encoder `call.rs`
/// uses for `ContractCallPrototype::input`.
pub fn input_json(
    args: &[(&str, midnight_contract::runtime::Value)],
    arg_types: &[(&str, compact_codegen::ir::TypeRef)],
) -> Json {
    let input = midnight_contract::interpreter::encode_circuit_input(args, arg_types)
        .expect("case arguments encode at their declared types");
    aligned_value_to_json(&input)
}

/// Concatenated communication outputs, as `call.rs` builds
/// `ContractCallPrototype::output`.
pub fn output_json(result: &ExecutionResult) -> Json {
    let output: AlignedValue = if result.communication_outputs.is_empty() {
        AlignedValue::from(())
    } else {
        AlignedValue::concat(result.communication_outputs.iter())
    };
    aligned_value_to_json(&output)
}

/// Canonical state channel: the post-execution `StateValue` both as readable
/// JSON and as the hex of a normalized serialized `ContractState` (fresh
/// state carrying only `data`, so entry-point registration and balances do
/// not leak into the comparison).
pub fn state_report_json(sv: &StateValue<InMemoryDB>) -> Json {
    json!({
        "data": state_value_to_json(sv),
        "serialized": normalized_state_hex(sv),
    })
}

/// Hex of `tagged_serialize` over a `ContractState` holding only `sv`.
pub fn normalized_state_hex(sv: &StateValue<InMemoryDB>) -> String {
    let cs: ContractState<InMemoryDB> = ContractState::new(
        sv.clone(),
        midnight_storage::storage::HashMap::new(),
        midnight_bindgen_runtime::ContractMaintenanceAuthority::default(),
    );
    let mut buf = Vec::new();
    midnight_serialize::tagged_serialize(&cs, &mut buf)
        .expect("ContractState serialization is infallible");
    hex::encode(buf)
}

/// The full per-step report.
pub fn step_report(
    circuit: &str,
    args: &[(&str, midnight_contract::runtime::Value)],
    arg_types: &[(&str, compact_codegen::ir::TypeRef)],
    result: &ExecutionResult,
) -> Json {
    json!({
        "circuit": circuit,
        "input": input_json(args, arg_types),
        "output": output_json(result),
        "publicTranscript": public_transcript_json(result),
        "privateTranscriptOutputs": result
            .private_transcript_outputs
            .iter()
            .map(aligned_value_to_json)
            .collect::<Vec<_>>(),
        "state": state_report_json(&result.state.data.get()),
        "zswapOutputs": result
            .zswap_outputs
            .iter()
            .map(|out| json!({
                "coin": aligned_value_to_json(&out.coin.to_aligned_value()),
                "recipient": aligned_value_to_json(&out.recipient.to_aligned_value()),
            }))
            .collect::<Vec<_>>(),
    })
}
