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
