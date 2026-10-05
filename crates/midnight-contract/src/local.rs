//! Run circuits in process, with no chain, proof or wallet.

use std::time::{SystemTime, UNIX_EPOCH};

use midnight_base_crypto::time::Timestamp;
use midnight_typed_state::{ContractState, InMemoryDB};
use midnight_types::{CoinPublicKey, ContractAddress};

use crate::error::ContractError;
use crate::interpreter::{self, Env, Program};
use crate::runtime::{NoWitnesses, Value, WitnessProvider};

/// Run a pure circuit in process and return its value.
///
/// `circuit` must have its `pure` flag set. `program` holds the circuits,
/// witnesses and natives that its body calls. `args` are the circuit's
/// arguments as (name, value) pairs, keyed by the argument's source name.
///
/// # Errors
///
/// [`ContractError::Interpreter`] when the circuit fails, for example on a
/// failed `assert` or on a call that the interpreter cannot run.
///
/// # Panics
///
/// When the `pure` flag of `circuit` is not set.
pub fn run_pure(
    circuit: &compact_codegen::ir::Circuit,
    program: &Program<'_>,
    args: &[(&str, Value)],
) -> Result<Option<Value>, ContractError> {
    assert!(
        circuit.pure,
        "run_pure needs a pure circuit, got `{}`",
        circuit.name.name()
    );
    // The compiler calls a circuit pure only when it touches no public state
    // and calls no witness, so the state and the time cannot change its value.
    let result = interpreter::execute(
        circuit,
        program,
        ContractState::default(),
        args,
        Env::new(Timestamp::from_secs(0)),
    )?;
    Ok(result.result)
}

/// Runs one contract's circuits in process, call after call.
///
/// It needs no chain, proof or wallet. A `Runner` holds what a call reads and
/// changes: the contract state, the private state that the witnesses update,
/// the block time, the contract address and the caller's coin public key.
/// Each [`run`](Self::run) starts from the state that the last successful run
/// left. A failed run changes nothing.
///
/// It runs circuits only. It makes no Zswap offer, and it has no Dust and no
/// fees. A generated contract's `Simulator` wraps it with one typed method per
/// impure circuit.
pub struct Runner<Wp = NoWitnesses> {
    state: ContractState<InMemoryDB>,
    private_state: Vec<u8>,
    witnesses: Wp,
    block_time: Timestamp,
    address: ContractAddress,
    coin_public_key: Option<CoinPublicKey>,
}

impl Runner {
    /// A runner over `state` at `block_time`.
    ///
    /// It has no witnesses, an empty private state, the zero address and no
    /// coin public key. The block time floors to whole seconds, the unit of
    /// the chain's clock checks.
    ///
    /// # Panics
    ///
    /// When `block_time` is before the Unix epoch.
    pub fn new(state: ContractState<InMemoryDB>, block_time: SystemTime) -> Self {
        Self {
            state,
            private_state: Vec::new(),
            witnesses: NoWitnesses,
            block_time: block_timestamp(block_time),
            address: ContractAddress(midnight_helpers::HashOutput([0; 32])),
            coin_public_key: None,
        }
    }
}

impl<Wp> Runner<Wp> {
    /// Answer the circuits' witness calls with `witnesses`.
    ///
    /// The private state stays as it is.
    pub fn with_witnesses<W>(self, witnesses: W) -> Runner<W> {
        Runner {
            state: self.state,
            private_state: self.private_state,
            witnesses,
            block_time: self.block_time,
            address: self.address,
            coin_public_key: self.coin_public_key,
        }
    }

    /// Set the time that the circuits' clock checks compare against. It
    /// floors to whole seconds.
    ///
    /// # Panics
    ///
    /// When `time` is before the Unix epoch.
    pub fn set_block_time(&mut self, time: SystemTime) {
        self.block_time = block_timestamp(time);
    }

    /// Set the address that `kernel.self()` reads.
    pub fn set_address(&mut self, address: ContractAddress) {
        self.address = address;
    }

    /// Set the caller's coin public key, which `ownPublicKey()` returns.
    pub fn set_coin_public_key(&mut self, key: CoinPublicKey) {
        self.coin_public_key = Some(key);
    }

    /// The contract state after the last successful run.
    pub fn state(&self) -> &ContractState<InMemoryDB> {
        &self.state
    }
}

impl<Wp: WitnessProvider> Runner<Wp> {
    /// Run `circuit` and return its value.
    ///
    /// `program` holds the circuits, witnesses and natives that its body
    /// calls. `args` are the circuit's arguments as (name, value) pairs, keyed
    /// by the argument's source name. On success, the runner keeps the
    /// contract state and the private state after the call.
    ///
    /// # Errors
    ///
    /// [`ContractError::Interpreter`] when the circuit or a witness fails, for
    /// example on a failed `assert`. With no coin public key set, a circuit
    /// that calls `ownPublicKey()` fails with this error. The runner keeps the
    /// state from before the call.
    pub fn run(
        &mut self,
        circuit: &compact_codegen::ir::Circuit,
        program: &Program<'_>,
        args: &[(&str, Value)],
    ) -> Result<Option<Value>, ContractError> {
        // The witnesses write the buffer as they run, so a call that fails
        // after a witness must not reach `self.private_state`.
        let mut private_state = self.private_state.clone();
        let result = crate::call::run_call(
            circuit,
            program,
            &self.state,
            self.block_time,
            self.address,
            args,
            &self.witnesses,
            Some(&mut private_state),
            self.coin_public_key,
        )?;
        self.state = result.state;
        self.private_state = private_state;
        Ok(result.result)
    }
}

// The private state can hold secret keys, so `Debug` leaves it out.
impl<Wp> std::fmt::Debug for Runner<Wp> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("block_time", &self.block_time)
            .field("address", &self.address)
            .field("coin_public_key", &self.coin_public_key)
            .finish_non_exhaustive()
    }
}

fn block_timestamp(time: SystemTime) -> Timestamp {
    let since_epoch = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| panic!("the block time {time:?} is before the Unix epoch"));
    Timestamp::from_secs(since_epoch.as_secs())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    #[should_panic(expected = "before the Unix epoch")]
    fn a_block_time_before_the_epoch_panics() {
        let _ = Runner::new(
            ContractState::default(),
            UNIX_EPOCH - Duration::from_secs(1),
        );
    }
}
