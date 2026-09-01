//! What a circuit call needs, in terms no generation owns.

use compact_codegen::ir;
pub use compact_values::ArgValue;

/// A circuit call on a deployed contract.
///
/// The IR comes from `compact-codegen`, which names no ledger crate, so the
/// same values describe the call on either generation. The interpreter that
/// runs them is per-generation, and the backend builds it.
pub struct CircuitCall<'a> {
    /// The contract to call.
    pub address: &'a str,
    /// The directory holding the contract's compiled artifacts.
    pub zk_config_dir: &'a str,
    /// The circuit to run, resolved by the caller.
    ///
    /// The artifact names a circuit twice: an exported name the chain keys its
    /// entry point on, and a mangled internal one the program is keyed by.
    /// Passing the definition avoids guessing which lookup applies.
    pub circuit: &'a ir::Circuit,
    /// Every circuit the contract declares, which the program needs to resolve
    /// calls the body makes.
    pub circuits: &'a [ir::Circuit],
    /// The witnesses the contract declares.
    pub witnesses: &'a [ir::Witness],
    /// The natives the contract declares.
    pub natives: &'a [ir::Native],
    /// The circuit's exported name, which the chain keys its entry point on.
    pub circuit_name: &'a str,
    /// The circuit's arguments, by parameter name.
    pub args: &'a [(&'a str, ArgValue)],
}
