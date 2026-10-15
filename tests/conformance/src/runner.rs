//! Drive the Rust IR interpreter over a case file.

use std::collections::HashMap;
use std::sync::Mutex;

use compact_codegen::arg_types::circuit_arg_types;
use compact_codegen::ir::Type;
use compact_codegen::types::ContractInfo;
use midnight_base_crypto::time::Timestamp;
use midnight_coin_structure::coin::{
    PublicKey as CoinPublicKey, ShieldedTokenType, TokenType, UnshieldedTokenType,
};
use midnight_contract::interpreter;
use midnight_contract::runtime::{
    ExecutionResult, InterpreterError, Value, WitnessContext, WitnessOutcome, WitnessProvider,
};
use midnight_typed_state::{
    ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue, StorageHashMap,
};
use serde_json::Value as Json;

use crate::corpus::corpus_dir;
use crate::report::step_report;
use crate::state_json::hash_from_json;
use crate::tagged::{parse_decimal, to_interpreter_value};

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
    /// The fixture `name`, read from its `compiler/analyzed-ir.sexp` in the
    /// `fixtures/` directory of the corpus.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or does not parse.
    pub fn open(name: &str) -> Result<Self, String> {
        let path = corpus_dir()
            .join("fixtures")
            .join(name)
            .join("compiler/analyzed-ir.sexp");
        let info = compact_codegen::artifact::load(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { info })
    }
}

/// Run one step (a single circuit invocation) of a case at `block_time`, as
/// the caller that holds `coin_public_key`, and build its canonical step
/// report.
pub fn run_step(
    fixture: &Fixture,
    circuit: &str,
    state: ContractState<InMemoryDB>,
    args_tagged: &[Json],
    witnesses: &ScriptedWitnesses,
    block_time: Timestamp,
    coin_public_key: Option<CoinPublicKey>,
) -> Result<(Json, ExecutionResult), String> {
    let entry = fixture
        .info
        .circuits
        .iter()
        .find(|c| c.name == circuit)
        .ok_or_else(|| format!("circuit {circuit} not found"))?;
    let arg_types = circuit_arg_types(entry.arguments());

    if args_tagged.len() != arg_types.len() {
        return Err(format!(
            "circuit {circuit} expects {} argument(s), case has {}",
            arg_types.len(),
            args_tagged.len()
        ));
    }
    let args: Vec<(&str, Value)> = arg_types
        .iter()
        .zip(args_tagged)
        .map(|((name, _ty), tagged)| Ok((name.as_str(), to_interpreter_value(tagged)?)))
        .collect::<Result<Vec<_>, String>>()?;

    let program = interpreter::Program::new(
        &fixture.info.helpers,
        &fixture.info.witnesses,
        &fixture.info.natives,
    );
    let result = interpreter::execute(
        &entry.def,
        &program,
        state,
        &args,
        interpreter::Env {
            witnesses,
            coin_public_key,
            ..interpreter::Env::new(block_time)
        },
    )
    .map_err(|e| format!("circuit {circuit}: {e}"))?;

    let type_refs: Vec<(&str, Type)> = arg_types
        .iter()
        .map(|(n, t)| (n.as_str(), t.clone()))
        .collect();
    let report = step_report(circuit, &args, &type_refs, &result);
    Ok((report, result))
}

/// Wrap a bare `StateValue` into the harness's normalized `ContractState`,
/// which holds the case's balance.
pub fn state_from_value(
    sv: StateValue<InMemoryDB>,
    balance: StorageHashMap<TokenType, u128, InMemoryDB>,
) -> ContractState<InMemoryDB> {
    ContractState {
        balance,
        ..ContractState::new(
            sv,
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }
}

/// Parse a case's `balance`: `[[tokenType, "<decimal>"], ...]`, with each
/// token type in the TS runtime's `{tag, raw}` form. `None` is an empty
/// balance.
pub fn balance_from_json(
    json: Option<&Json>,
) -> Result<StorageHashMap<TokenType, u128, InMemoryDB>, String> {
    let mut balance = StorageHashMap::new();
    let Some(json) = json else {
        return Ok(balance);
    };
    let entries = json
        .as_array()
        .ok_or_else(|| format!("balance must be an array: {json}"))?;
    for entry in entries {
        let pair = entry
            .as_array()
            .filter(|pair| pair.len() == 2)
            .ok_or_else(|| format!("balance entry must be a [tokenType, amount] pair: {entry}"))?;
        let amount = parse_decimal(&pair[1]).map_err(|e| format!("balance amount {entry}: {e}"))?;
        balance = balance.insert(token_type_from_json(&pair[0])?, amount);
    }
    Ok(balance)
}

/// Parse a case's `coinPublicKey`: the key's 32 bytes as hex. `None` is the
/// zero key.
pub fn coin_public_key_from_json(json: Option<&Json>) -> Result<CoinPublicKey, String> {
    match json {
        Some(json) => hash_from_json(json)
            .map(CoinPublicKey)
            .map_err(|e| format!("coinPublicKey: {e}")),
        None => Ok(CoinPublicKey::default()),
    }
}

fn token_type_from_json(json: &Json) -> Result<TokenType, String> {
    let raw = || hash_from_json(&json["raw"]).map_err(|e| format!("token type {json} raw: {e}"));
    match json["tag"].as_str() {
        Some("unshielded") => Ok(TokenType::Unshielded(UnshieldedTokenType(raw()?))),
        Some("shielded") => Ok(TokenType::Shielded(ShieldedTokenType(raw()?))),
        Some("dust") => Ok(TokenType::Dust),
        _ => Err(format!("unknown token type: {json}")),
    }
}
