//! Conformance harness support: run a compiled circuit through the Rust IR
//! interpreter and normalize the outcome into the same canonical JSON report
//! the TS driver (`ts-driver/driver.mjs`) emits from the canonical
//! `@midnight-ntwrk/compact-runtime`. See
//! `docs/plans/2026-07-05-conformance-harness-design.md`.

pub mod report;
pub mod runner;
pub mod state_json;
pub mod tagged;

#[cfg(test)]
mod shape_tests;
