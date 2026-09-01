//! Turn the `ledger-9` feature into the `ledger_9` cfg the source reads.
//!
//! The per-generation shims set the same cfg directly, and a cfg cannot be
//! switched on from `--features`. That keeps a workspace-wide
//! `--features ...ledger-9` from reaching a shim pinned to the older ledger.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(ledger_9)");
    if std::env::var_os("CARGO_FEATURE_LEDGER_9").is_some() {
        println!("cargo::rustc-cfg=ledger_9");
    }
}
