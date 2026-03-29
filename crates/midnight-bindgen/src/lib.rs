//! Generate typed Rust bindings from Midnight Compact smart contracts.
//!
//! This crate provides a single dependency for generating type-safe Rust
//! bindings from a Compact compiler's `contract-info.json` output.
//!
//! # Usage
//!
//! ```ignore
//! // Generate bindings with a named module (recommended).
//! midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");
//!
//! use gateway::*;
//!
//! let ledger = Gateway::new(state);
//! let threshold: u8 = ledger.threshold()?;
//! ```
//!
//! # What gets generated
//!
//! - **Field constants** -- `FIELD_THRESHOLD`, `FIELD_VALIDATORS`, etc.
//! - **Data types** -- Structs and enums mirroring Compact type definitions
//! - **Ledger struct** -- Typed synchronous accessors for each ledger field (eager, full state in memory)
//! - **Lazy query struct** -- `{Name}Query<P>` with async accessors that fetch individual fields via RPC
//! - **Circuit call types** -- `*Call` structs and `*Return` type aliases
//!
//! # Lazy queries
//!
//! ```ignore
//! use gateway::GatewayQuery;
//!
//! let query = GatewayQuery::new(provider, "contract_address");
//! let threshold: u8 = query.threshold().await?;
//! ```
//!
//! The provider must implement [`lazy::StateQueryProvider`]. Lazy accessors
//! go directly to the node RPC -- no indexer required.

/// Re-export the proc macro.
pub use midnight_bindgen_macro::contract;

/// Re-export the runtime so generated code can use `midnight_bindgen::*`.
pub use midnight_bindgen_runtime::*;
