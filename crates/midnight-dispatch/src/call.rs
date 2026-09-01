//! What a circuit call needs, in terms no generation owns.

use compact_codegen::ir;

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
    /// The circuits the contract declares.
    pub circuits: &'a [ir::Circuit],
    /// The witnesses the contract declares.
    pub witnesses: &'a [ir::Witness],
    /// The natives the contract declares.
    pub natives: &'a [ir::Native],
    /// Which circuit to call.
    pub circuit_name: &'a str,
}
