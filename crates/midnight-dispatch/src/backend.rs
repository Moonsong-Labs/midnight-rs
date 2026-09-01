//! One API over the ledger generations this build carries.

use async_trait::async_trait;

use compact_values::Witnesses;

use crate::{
    ArgValue, CircuitCall, Error, Generation, Health, Landed, Opening, OpeningField, Verdict,
};
use std::str::FromStr as _;

/// What a client needs from a chain, without naming a ledger generation.
///
/// A trait object rather than an enum: a later generation is one more
/// implementation, not an edit to every method here.
#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// The generation this backend speaks.
    fn generation(&self) -> Generation;

    /// The ledger version the node reports.
    async fn ledger_version(&self) -> Result<String, Error>;

    /// Node and indexer reachability.
    async fn health(&self) -> Result<Health, Error>;

    /// Combine transactions into one. Bytes in, bytes out, so no conversion.
    async fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, Error>;

    /// Fund a transaction from the attached wallet. Bytes in, bytes out.
    async fn balance_transaction(&self, tx: &[u8]) -> Result<Vec<u8>, Error>;

    /// Attach a wallet synced from `seed`, which a deploy or a transfer needs
    /// to fund itself.
    ///
    /// Takes the backend by value because attaching replaces the provider.
    async fn with_wallet(
        self: Box<Self>,
        indexer_url: &str,
        seed: [u8; 32],
        network: &str,
    ) -> Result<Box<dyn Backend>, Error>;

    /// A contract's state, hex-encoded as the indexer serves it.
    ///
    /// The encoding is the ledger's own tagged form, and `StateValue` carries
    /// the same tag in both generations, so this crosses the boundary without
    /// conversion.
    async fn contract_state(&self, address: &str) -> Result<Option<String>, Error>;

    /// Deploy a contract from a compiled-artifact directory, and return its
    /// address.
    async fn deploy(&self, zk_config_dir: &str, opening: Opening) -> Result<String, Error>;

    /// Call a circuit on a deployed contract, and wait for it to land.
    ///
    /// The program is built here from the IR the caller passes, because
    /// `compact-codegen` names no ledger crate and its IR is the same on
    /// every generation, while the interpreter that runs it is not.
    async fn call(&self, call: CircuitCall<'_>) -> Result<Landed, Error>;

    /// Submit a transaction and wait for it to be finalized.
    ///
    /// Waiting is part of this call because the handle a submission returns is
    /// the generation's own, holding a node subscription that cannot cross
    /// this boundary.
    async fn submit_and_wait(&self, tx: &[u8]) -> Result<Landed, Error>;
}

/// Turn a neutral argument into one generation's runtime value.
///
/// The interpreter's value type carries a ledger state variant this cannot
/// produce, which is what keeps the neutral type free of a generation.
macro_rules! to_runtime_value_for {
    ($name:ident, $contract:ident) => {
        fn $name(value: &ArgValue) -> $contract::runtime::Value {
            match value {
                ArgValue::Bool(b) => $contract::runtime::Value::Bool(*b),
                ArgValue::Integer(i) => $contract::runtime::Value::Integer(*i),
                ArgValue::Aligned(a) => $contract::runtime::Value::AlignedValue(a.clone()),
                ArgValue::Struct(fields) => $contract::runtime::Value::Struct(
                    fields.iter().map(|(k, v)| (k.clone(), $name(v))).collect(),
                ),
                ArgValue::Tuple(items) => {
                    $contract::runtime::Value::Tuple(items.iter().map($name).collect())
                }
                ArgValue::Void => $contract::runtime::Value::Void,
            }
        }
    };
}

/// Present a neutral witness provider as one generation's own.
///
/// The interpreter carries private state as opaque bytes, so only the values
/// need converting. A value that has no neutral form is a genuine failure
/// rather than an unknown witness: reporting it as unknown would make the
/// interpreter fall through to a builtin and quietly compute the wrong thing.
macro_rules! witness_adapter_for {
    ($name:ident, $contract:ident, $to_runtime:ident, $from_runtime:ident) => {
        struct $name<'a>(&'a dyn Witnesses);

        impl $contract::runtime::WitnessProvider for $name<'_> {
            fn call_witness(
                &self,
                ctx: &mut $contract::runtime::WitnessContext<'_>,
                name: &str,
                args: &[$contract::runtime::Value],
            ) -> Result<$contract::runtime::WitnessOutcome, $contract::runtime::InterpreterError>
            {
                let args = args
                    .iter()
                    .map($from_runtime)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e: String| $contract::runtime::InterpreterError::Witness(e))?;
                let mut state = ctx.private_state().to_vec();
                let outcome = self
                    .0
                    .call(&mut state, name, &args)
                    .map_err($contract::runtime::InterpreterError::Witness)?;
                ctx.set_private_state(state);
                Ok(match outcome {
                    Some(value) => $contract::runtime::WitnessOutcome::Value($to_runtime(&value)),
                    None => $contract::runtime::WitnessOutcome::Unknown,
                })
            }
        }
    };
}

/// Turn one generation's runtime value into the neutral form.
///
/// A ledger state value has no neutral form; it is how a running circuit holds
/// a cell, never something a witness hands a caller.
macro_rules! from_runtime_value_for {
    ($name:ident, $contract:ident) => {
        fn $name(value: &$contract::runtime::Value) -> Result<ArgValue, String> {
            Ok(match value {
                $contract::runtime::Value::Bool(b) => ArgValue::Bool(*b),
                $contract::runtime::Value::Integer(i) => ArgValue::Integer(*i),
                $contract::runtime::Value::AlignedValue(a) => ArgValue::Aligned(a.clone()),
                $contract::runtime::Value::Struct(fields) => ArgValue::Struct(
                    fields
                        .iter()
                        .map(|(k, v)| $name(v).map(|v| (k.clone(), v)))
                        .collect::<Result<_, _>>()?,
                ),
                $contract::runtime::Value::Tuple(items) => {
                    ArgValue::Tuple(items.iter().map($name).collect::<Result<_, _>>()?)
                }
                $contract::runtime::Value::Void => ArgValue::Void,
                $contract::runtime::Value::StateValue(_) => {
                    return Err("a witness cannot take a ledger state value".into());
                }
            })
        }
    };
}

/// Implement [`Backend`] over one generation's provider.
///
/// Each generation's `MidnightProvider` is a distinct type with the same shape,
/// so the bodies are identical and only the crate differs. Writing them out
/// per generation is what lets the conversion to the neutral types happen at
/// this boundary and nowhere else.
macro_rules! backend_over {
    ($module:ident, $contract:ident, $interpreter:ident, $wallet:ident, $to_value:ident, $witnesses:ident, $generation:expr) => {
        #[async_trait]
        impl Backend for $module::MidnightProvider {
            fn generation(&self) -> Generation {
                $generation
            }

            async fn ledger_version(&self) -> Result<String, Error> {
                self.ledger_version()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn with_wallet(
                self: Box<Self>,
                indexer_url: &str,
                seed: [u8; 32],
                network: &str,
            ) -> Result<Box<dyn Backend>, Error> {
                let network = $module::Network::from_str(network)
                    .map_err(|_| Error::UnknownNetwork(network.to_owned()))?;
                let wallet = $wallet::Wallet::sync(indexer_url, seed, network)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Box::new(
                    (*self).with_wallet($wallet::LocalWallet::new(wallet)),
                ))
            }

            async fn deploy(&self, zk_config_dir: &str, opening: Opening) -> Result<String, Error> {
                let fields = opening
                    .fields
                    .into_iter()
                    .map(|field| match field {
                        OpeningField::Cell(value) => $contract::InitialField::Cell(value),
                        OpeningField::Counter(value) => $contract::InitialField::Counter(value),
                        OpeningField::Map => $contract::InitialField::Map,
                        OpeningField::List => $contract::InitialField::List,
                        OpeningField::MerkleTree => $contract::InitialField::MerkleTree,
                    })
                    .collect();
                let contract = $contract::Contract::deploy(self)
                    .with_initial_state($contract::InitialState::new(fields))
                    .with_zk_config(zk_config_dir)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(contract.address().to_owned())
            }

            async fn call(&self, call: CircuitCall<'_>) -> Result<Landed, Error> {
                let contract = $contract::Contract::at(self, call.address)
                    .with_zk_config(call.zk_config_dir)
                    .build();
                let program =
                    $interpreter::Program::new(call.circuits, call.witnesses, call.natives);

                let args: Vec<(&str, $contract::runtime::Value)> = call
                    .args
                    .iter()
                    .map(|(name, value)| (*name, $to_value(value)))
                    .collect();
                let outcome = contract
                    .call_with(
                        call.circuit,
                        &program,
                        call.circuit_name,
                        &args,
                        &$witnesses(call.private_state),
                        &[],
                        Default::default(),
                    )
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Landed {
                    block_hash: outcome.block_hash,
                    extrinsic_hash: outcome.extrinsic_hash,
                    transaction_hash: *outcome.transaction_hash.as_bytes(),
                    // A call that returned an outcome landed; the chain
                    // reports a rejection as an error instead.
                    verdict: Verdict::Success,
                })
            }

            async fn contract_state(&self, address: &str) -> Result<Option<String>, Error> {
                $module::Provider::get_contract_state(self, address, None)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
                // Sync on the provider: merging is local, with no chain call.
                self.merge_transactions(txs)
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn balance_transaction(&self, tx: &[u8]) -> Result<Vec<u8>, Error> {
                self.balance_transaction(tx)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn submit_and_wait(&self, tx: &[u8]) -> Result<Landed, Error> {
                let pending = self
                    .submit(tx)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                let (landed, _) = pending
                    .wait_finalized()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Landed {
                    block_hash: landed.block_hash,
                    extrinsic_hash: landed.extrinsic_hash,
                    transaction_hash: *landed.transaction_hash.as_bytes(),
                    verdict: match landed.verdict {
                        $module::Verdict::Success => Verdict::Success,
                        $module::Verdict::PartialSuccess => Verdict::PartialSuccess,
                        $module::Verdict::Failure => Verdict::Failure,
                    },
                })
            }

            async fn health(&self) -> Result<Health, Error> {
                let health = self
                    .health()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Health {
                    node_connected: health.node_connected,
                    indexer_connected: health.indexer_connected,
                    block_height: health.block_height,
                    peers: health.peers,
                    is_syncing: health.is_syncing,
                })
            }
        }
    };
}

to_runtime_value_for!(to_value_8, c8);
to_runtime_value_for!(to_value_9, c9);
from_runtime_value_for!(from_value_8, c8);
from_runtime_value_for!(from_value_9, c9);
witness_adapter_for!(Witnesses8, c8, to_value_8, from_value_8);
witness_adapter_for!(Witnesses9, c9, to_value_9, from_value_9);

backend_over!(p8, c8, i8_, w8, to_value_8, Witnesses8, Generation::Ledger8);
backend_over!(p9, c9, i9, w9, to_value_9, Witnesses9, Generation::Ledger9);

#[cfg(test)]
mod tests {
    use super::*;

    /// Each generation's provider reports its own generation, so a connection
    /// dispatches to the ledger the chain runs rather than a compiled-in one.
    /// Both are linked here: were they one crate, this could not compile.
    #[test]
    fn each_backend_reports_its_own_generation() {
        let eight =
            p8::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").expect("construct");
        let nine =
            p9::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").expect("construct");
        assert_eq!(Backend::generation(&eight), Generation::Ledger8);
        assert_eq!(Backend::generation(&nine), Generation::Ledger9);
    }

    /// The neutral surface is the same on both generations, so a caller can
    /// hold either behind one pointer. This is the property dispatch rests on:
    /// were the two providers not interchangeable here, `Client` could not
    /// choose between them at runtime.
    #[test]
    fn both_generations_satisfy_one_trait_object() {
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(
                p8::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1")
                    .expect("construct"),
            ),
            Box::new(
                p9::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1")
                    .expect("construct"),
            ),
        ];
        let seen: Vec<Generation> = backends.iter().map(|b| b.generation()).collect();
        assert_eq!(seen, vec![Generation::Ledger8, Generation::Ledger9]);
    }
}
