//! Runtime support library for compact-bindgen generated code.
//!
//! Provides state navigation helpers, typed accessors, error types, and
//! lazy query infrastructure used by the generated contract bindings.
//! Not intended for direct use --
//! depend on the `compact-bindgen` crate instead.
//!
//! The [`lazy`] module defines the [`lazy::StateQueryProvider`] trait and
//! helpers for per-field RPC queries (no indexer required).

mod accessors;
mod error;
mod nav;
mod reexports;

mod conversions;

pub use accessors::{ListAccessor, MapAccessor, MerkleTreeAccessor, SetAccessor};
pub use conversions::{Bytes, Vector};
pub use error::StateError;
pub use nav::{cell_value, get_field, get_field_path, variant_name};
pub use reexports::*;

pub mod lazy;

/// Re-export `hex` so generated code can use it without adding a direct dependency.
/// Internal — applications wanting hex utilities should depend on the `hex` crate directly.
#[doc(hidden)]
pub use hex;

/// Re-export `serde_json` so generated code can use it without adding a direct dependency.
/// Internal — applications wanting JSON utilities should depend on `serde_json` directly.
#[doc(hidden)]
pub use serde_json;

/// Re-export `serde` so generated witness adapters can name `serde::Serialize` /
/// `serde::de::DeserializeOwned` (the bounds on a witness's `PrivateState`) without the
/// consuming crate adding a direct dependency. Internal.
#[doc(hidden)]
pub use serde;
