//! A generated builder does nothing until a caller awaits, sends, builds or
//! simulates it, so dropping one must fail under `unused_must_use`.

#![deny(unused_must_use)]

compact_bindgen::contract!(
    "../../../../crates/midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
);

use compact_bindgen::midnight_contract::AsMidnightProvider;

fn drop_deploy<P: AsMidnightProvider>(provider: P) {
    Contract::deploy(provider);
}

fn drop_connect<P: AsMidnightProvider>(provider: P) {
    Contract::at(provider, "00");
}

fn drop_call<P: AsMidnightProvider>(contract: &Contract<P>) {
    contract.circuits().increment();
}

fn main() {}
