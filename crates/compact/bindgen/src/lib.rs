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
//! # fn main() {}
//! ```
//!
//! # What gets generated
//!
//! - **Field constants** -- `FIELD_THRESHOLD`, `FIELD_VALIDATORS`, etc.
//! - **Data types** -- Structs and enums mirroring Compact type definitions
//! - **Ledger struct** -- Typed synchronous accessors for each ledger field (eager, full state in memory)
//! - **Lazy query struct** -- `{Name}Query<P>` with async accessors that fetch individual fields via RPC
//! - **Circuit call types** -- `*Call` structs and `*Return` type aliases
//! - **Pure circuits** -- a `pure_circuits` module, with one function per exported pure circuit that runs it in process with no chain, proof or wallet
//! - **Deploy and call** -- `Contract::deploy`, which runs the contract's constructor and deploys the state that it leaves, `Contract::at`, and `Circuits`, with one call builder per impure circuit
//! - **Simulator** -- `Simulator`, which runs the impure circuits, and the constructor with `run_constructor`, in process with no chain, proof or wallet. The caller sets the block time, the address and the coin public key, and the ledger and the private state carry from call to call.
//! - **ZK config directory** -- `ZK_CONFIG_DIR`, the compactc output directory to pass to `with_zk_config`, for an artifact at `compiler/analyzed-ir.sexp` (see [`contract!`])
//!
//! # Lazy queries
//!
//! ```no_run
//! # compact_bindgen::contract!(
//! #     Gateway,
//! #     "../../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp"
//! # );
//! # use compact_bindgen::lazy;
//! use gateway::GatewayQuery;
//!
//! # async fn read<P: lazy::StateQueryProvider>(provider: P) -> Result<(), lazy::ContractError> {
//! let query = GatewayQuery::new(provider, "contract_address", None);
//! let threshold: u8 = query.threshold().await?;
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! The provider must implement [`lazy::StateQueryProvider`]. Lazy accessors
//! go directly to the node RPC. They need no indexer. The last argument of
//! `new` is a block hash that pins every read to that block. `None` reads the
//! latest state.

/// Re-export the proc macro.
pub use compact_bindgen_macro::contract;

/// Re-export the runtime so generated code can use `compact_bindgen::*`.
pub use midnight_typed_state::*;

/// Re-export of `midnight_contract` so the macro's generated code can reach
/// `compact_bindgen::midnight_contract::*` (and direct callers can too,
/// without an extra dep on midnight-contract).
pub use midnight_contract;
