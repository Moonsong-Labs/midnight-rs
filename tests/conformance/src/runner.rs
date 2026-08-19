//! Drive the Rust IR interpreter over a case file.

use std::collections::HashMap;
use std::sync::Mutex;

use compact_codegen::arg_types::circuit_arg_types;
use compact_codegen::ir::Type;
use compact_codegen::types::ContractInfo;
use midnight_contract::interpreter;
use midnight_contract::runtime::{
    ExecutionResult, InterpreterError, Value, WitnessContext, WitnessOutcome, WitnessProvider,
};
use midnight_typed_state::{ContractState, InMemoryDB, StateValue};
use serde_json::Value as Json;

use crate::tagged::to_interpreter_value;

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

/// Everything the interpreter needs from a fixture's `analyzed-ir.sexp`.
pub struct Fixture {
    pub info: ContractInfo,
}

impl Fixture {
    pub fn load(analyzed_ir_text: &str) -> Result<Self, String> {
        let info =
            compact_codegen::artifact::load_str(analyzed_ir_text).map_err(|e| e.to_string())?;
        Ok(Self { info })
    }

    /// The circuit's IR body, as the SDK's call path receives it (generated
    /// bindings embed the same typed value as a constructor).
    pub fn circuit(&self, circuit: &str) -> Result<&compact_codegen::ir::Circuit, String> {
        self.info
            .circuits
            .iter()
            .find(|c| c.name == circuit)
            .map(|c| &c.def)
            .ok_or_else(|| format!("circuit {circuit} not found"))
    }

    /// Declared argument and result types plus inline struct/enum defs for a
    /// circuit.
    pub fn circuit_defs(&self, circuit: &str) -> Result<CircuitMeta, String> {
        let entry = self
            .info
            .circuits
            .iter()
            .find(|c| c.name == circuit)
            .ok_or_else(|| format!("circuit {circuit} not found"))?;
        let arg_types = circuit_arg_types(entry.arguments());
        let result_type = entry.result_type().resolved().clone();
        Ok(CircuitMeta {
            arg_types,
            result_type,
        })
    }
}

/// A circuit's declared types and the definitions its IR references.
pub struct CircuitMeta {
    pub arg_types: Vec<(String, Type)>,
    pub result_type: Type,
}

/// Run one step (a single circuit invocation) of a case.
pub fn run_step(
    fixture: &Fixture,
    circuit: &str,
    state: ContractState<InMemoryDB>,
    args_tagged: &[Json],
    witnesses: &ScriptedWitnesses,
) -> Result<(Vec<(String, Value)>, ExecutionResult), String> {
    let circuit_def = fixture.circuit(circuit)?;
    let meta = fixture.circuit_defs(circuit)?;

    if args_tagged.len() != meta.arg_types.len() {
        return Err(format!(
            "circuit {circuit} expects {} argument(s), case has {}",
            meta.arg_types.len(),
            args_tagged.len()
        ));
    }
    let args: Vec<(String, Value)> = meta
        .arg_types
        .iter()
        .zip(args_tagged)
        .map(|((name, _ty), tagged)| Ok((name.clone(), to_interpreter_value(tagged)?)))
        .collect::<Result<Vec<_>, String>>()?;

    let arg_refs: Vec<(&str, Value)> = args.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();

    let program = interpreter::Program::new(
        &fixture.info.helpers,
        &fixture.info.witnesses,
        &fixture.info.natives,
    );
    let result = interpreter::execute_with_owned(
        circuit_def,
        &program,
        state,
        &arg_refs,
        witnesses,
        None,
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
        midnight_typed_state::ContractMaintenanceAuthority::default(),
    )
}
