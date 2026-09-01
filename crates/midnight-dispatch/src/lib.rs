//! Speak to a chain without being told which ledger it runs.
//!
//! A client built for one ledger generation cannot transact on a chain running
//! the other: the transaction encodings differ, and the node rejects what it
//! cannot parse. A new ledger reaches the testnets before mainnet, so at every
//! release there is a window where the networks disagree.
//!
//! This crate holds both generations at once and picks between them from what
//! the node reports, so a caller names no generation.

mod generation;

pub use generation::{Generation, GenerationError, generation_of};

mod backend;
mod call;
mod client;
mod error;
mod health;
mod opening;
mod transaction;

pub use call::{ArgValue, CircuitCall};
pub use client::Client;
pub use error::Error;
pub use health::Health;
pub use opening::{Opening, OpeningField};
pub use transaction::{Landed, Verdict};
