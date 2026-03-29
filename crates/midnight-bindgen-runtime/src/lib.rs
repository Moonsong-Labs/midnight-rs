//! Runtime support library for midnight-bindgen generated code.
//!
//! Provides state navigation helpers, typed accessors, error types, and
//! lazy query infrastructure used by the generated contract bindings.
//! Not intended for direct use --
//! depend on [`midnight-bindgen`](https://crates.io/crates/midnight-bindgen) instead.
//!
//! The [`lazy`] module defines the [`lazy::StateQueryProvider`] trait and
//! helpers for per-field RPC queries (no indexer required).

mod accessors;
mod error;
mod nav;
mod reexports;

mod conversions;

pub use accessors::{ListAccessor, MapAccessor, MerkleTreeAccessor, SetAccessor};
pub use conversions::Bytes;
pub use error::StateError;
pub use nav::{cell_value, get_field, get_field_path, variant_name};
pub use reexports::*;

pub mod lazy;

/// Re-export `hex` so generated code can use it without adding a direct dependency.
pub use hex;
