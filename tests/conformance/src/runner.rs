//! Drive the Rust IR interpreter over a case file.

use std::collections::HashMap;
use std::sync::Mutex;

use compact_codegen::arg_types::{circuit_arg_types, collect_argument_defs};
use compact_codegen::ir::{CircuitIrBody, EnumDef, StructDef, TypeRef};
use compact_codegen::types::ContractInfo;
use midnight_bindgen_runtime::{ContractState, InMemoryDB, StateValue};
use midnight_contract::interpreter::{
    self, ExecutionResult, InterpreterError, Value, WitnessContext, WitnessOutcome,
    WitnessProvider,
};
use serde_json::Value as Json;

use crate::tagged::to_interpreter_value;

/// A parsed case file (`cases/<fixture>/<case>.json`).
pub struct Case {
    pub fixture: String,
    pub name: String,
    pub json: Json,
}

/// A witness provider that replays scripted values: for each witness name, a
/// queue of tagged values consumed one per call. The TS driver replays the
/// same script, so both executors see identical private inputs.
pub struct ScriptedWitnesses {
    queues: Mutex<HashMap<String, Vec<Json>>>,
}

impl ScriptedWitnesses {
    /// Build from a case's `witnesses` object: `{name: [tagged values...]}`.
    pub fn from_json(json: Option<&Json>) -> Result<Self, String> {
        let mut queues = HashMap::new();
        if let Some(obj) = json {
            let map = obj
                .as_object()
                .ok_or_else(|| format!("witnesses must be an object: {obj}"))?;
            for (name, values) in map {
                let list = values
                    .as_array()
                    .ok_or_else(|| format!("witness {name} script must be an array"))?;
                // Reversed so calls can pop() in order.
                queues.insert(name.clone(), list.iter().rev().cloned().collect());
            }
        }
        Ok(Self {
            queues: Mutex::new(queues),
        })
    }

    /// Fail the case when a scripted value was never consumed: that means
    /// the two executors disagreed on how many witness calls the circuit
    /// makes, which the transcript diff alone might miss.
    pub fn assert_drained(&self) -> Result<(), String> {
        let queues = self.queues.lock().expect("no poisoned locks in tests");
        for (name, queue) in queues.iter() {
            if !queue.is_empty() {
                return Err(format!(
                    "witness {name} has {} unconsumed scripted value(s)",
                    queue.len()
                ));
            }
        }
        Ok(())
    }
}

impl WitnessProvider for ScriptedWitnesses {
    fn call_witness(
        &self,
        _ctx: &mut WitnessContext<'_>,
        name: &str,
        _args: &[Value],
    ) -> Result<WitnessOutcome, InterpreterError> {
        let mut queues = self.queues.lock().expect("no poisoned locks in tests");
        match queues.get_mut(name) {
            Some(queue) => {
                let tagged = queue.pop().ok_or_else(|| {
                    InterpreterError::Witness(format!("witness {name}: script exhausted"))
                })?;
                let value = to_interpreter_value(&tagged)
                    .map_err(|e| InterpreterError::Witness(format!("witness {name}: {e}")))?;
                Ok(WitnessOutcome::Value(value))
            }
            None => Ok(WitnessOutcome::Unknown),
        }
    }
}

/// Everything the interpreter needs from a fixture's `contract-info.json`.
pub struct Fixture {
    pub info: ContractInfo,
    raw: Json,
}

impl Fixture {
    pub fn load(contract_info_json: &str) -> Result<Self, String> {
        let info: ContractInfo =
            serde_json::from_str(contract_info_json).map_err(|e| format!("contract-info: {e}"))?;
        let raw: Json =
            serde_json::from_str(contract_info_json).map_err(|e| format!("contract-info: {e}"))?;
        Ok(Self { info, raw })
    }

    /// The circuit's IR body, from the raw JSON (mirrors how the SDK's call
    /// path deserializes it).
    pub fn circuit_ir(&self, circuit: &str) -> Result<CircuitIrBody, String> {
        let circuits = self.raw["circuits"]
            .as_array()
            .ok_or("contract-info has no circuits")?;
        let entry = circuits
            .iter()
            .find(|c| c["name"].as_str() == Some(circuit))
            .ok_or_else(|| format!("circuit {circuit} not found"))?;
        serde_json::from_value(entry["ir"].clone()).map_err(|e| format!("circuit {circuit}: {e}"))
    }

    /// Declared argument types plus inline struct/enum defs for a circuit.
    pub fn circuit_defs(
        &self,
        circuit: &str,
    ) -> Result<(Vec<(String, TypeRef)>, Vec<StructDef>, Vec<EnumDef>), String> {
        let entry = self
            .info
            .circuits
            .iter()
            .find(|c| c.name == circuit)
            .ok_or_else(|| format!("circuit {circuit} not found"))?;
        let arg_types = circuit_arg_types(&entry.arguments);
        let mut structs = self.info.structs.clone();
        let mut enums = Vec::new();
        collect_argument_defs(&entry.arguments, &mut structs, &mut enums);
        Ok((arg_types, structs, enums))
    }
}

/// Run one step (a single circuit invocation) of a case.
pub fn run_step(
    fixture: &Fixture,
    circuit: &str,
    state: ContractState<InMemoryDB>,
    args_tagged: &[Json],
    witnesses: &ScriptedWitnesses,
) -> Result<(Vec<(String, Value)>, ExecutionResult), String> {
    let ir = fixture.circuit_ir(circuit)?;
    let (arg_types, structs, enums) = fixture.circuit_defs(circuit)?;

    if args_tagged.len() != arg_types.len() {
        return Err(format!(
            "circuit {circuit} expects {} argument(s), case has {}",
            arg_types.len(),
            args_tagged.len()
        ));
    }
    let args: Vec<(String, Value)> = arg_types
        .iter()
        .zip(args_tagged)
        .map(|((name, _ty), tagged)| Ok((name.clone(), to_interpreter_value(tagged)?)))
        .collect::<Result<Vec<_>, String>>()?;

    let arg_refs: Vec<(&str, Value)> = args
        .iter()
        .map(|(n, v)| (n.as_str(), v.clone()))
        .collect();
    let type_refs: Vec<(&str, TypeRef)> = arg_types
        .iter()
        .map(|(n, t)| (n.as_str(), t.clone()))
        .collect();

    let result = interpreter::execute_with_owned(
        &ir,
        state,
        &arg_refs,
        &type_refs,
        witnesses,
        None,
        &fixture.info.helpers,
        &structs,
        &enums,
        None,
    )
    .map_err(|e| format!("circuit {circuit}: {e}"))?;

    Ok((args, result))
}

/// Wrap a bare `StateValue` into the harness's normalized `ContractState`.
pub fn state_from_value(sv: StateValue<InMemoryDB>) -> ContractState<InMemoryDB> {
    ContractState::new(
        sv,
        midnight_storage::storage::HashMap::new(),
        midnight_bindgen_runtime::ContractMaintenanceAuthority::default(),
    )
}
