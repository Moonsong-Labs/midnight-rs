//! Pin this shim to ledger 8.
//!
//! A cfg rather than a feature: `--features` reaches every member of a
//! workspace build, and would otherwise switch this shim onto the other
//! generation.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(ledger_9)");
}
