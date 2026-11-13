//! Generate typed Rust bindings from Midnight Compact smart contracts.
//!
//! This crate provides a single dependency for generating type-safe Rust
//! bindings from a Compact compiler's `analyzed-ir.sexp` output.
//!
//! # Usage
//!
//! ```no_run
//! // Generate bindings with a named module (recommended).
//! compact_bindgen::contract!(
//!     Gateway,
//!     "../../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp"
//! );
//!
//! use gateway::*;
//!
//! # use compact_bindgen::{ContractState, InMemoryDB, StateError};
//! # fn read(state: ContractState<InMemoryDB>) -> Result<(), StateError> {
//! let ledger = Gateway::new(state);
//! let threshold: u8 = ledger.threshold()?;
//! # Ok(())
//! # }
//! # fn read_rpc(hex_state: &str) -> Result<(), StateError> {
//! let ledger = Gateway::from_hex(hex_state)?;
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! # What gets generated
//!
//! - **Field constants** -- `FIELD_THRESHOLD`, `FIELD_VALIDATORS`, etc.
//! - **Data types** -- Structs and enums mirroring Compact type definitions
//! - **Ledger struct** -- Typed synchronous accessors for each ledger field (eager, full state in memory)
//! - **Return types** -- `*Return` type aliases for the result type of each circuit and witness
//! - **Pure circuits** -- a `pure_circuits` module, with one function per exported pure circuit that runs it in process with no chain, proof or wallet
//! - **Deploy and call** -- `Contract::deploy`, which runs the contract's constructor and deploys the state that it leaves, `Contract::at`, and `Circuits`, with one call builder per impure circuit
//! - **Simulator** -- `Simulator`, which runs the impure circuits, and the constructor with `run_constructor`, in process with no chain, proof or wallet. The caller sets the block time, the address and the coin public key, and the ledger and the private state carry from call to call.
//! - **ZK config directory** -- `ZK_CONFIG_DIR`, the compactc output directory to pass to `with_zk_config`, for an artifact at `compiler/analyzed-ir.sexp` (see [`contract!`])

/// Re-export the proc macro.
pub use compact_bindgen_macro::contract;

/// Re-export the runtime so generated code can use `compact_bindgen::*`.
pub use midnight_typed_state::*;

// Re-export of `midnight_contract` so the macro's generated code can reach
// `compact_bindgen::midnight_contract::*`. Hidden from rustdoc because it is
// not part of the user-facing API.
#[doc(hidden)]
pub use midnight_contract;
