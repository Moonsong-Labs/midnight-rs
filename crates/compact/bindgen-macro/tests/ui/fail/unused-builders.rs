//! A generated builder does nothing until a caller awaits, sends or builds
//! it, so dropping one must fail under `unused_must_use`.

#![deny(unused_must_use)]

compact_bindgen::contract!(
    "../../../../crates/midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
);

use compact_bindgen::midnight_contract::{AsMidnightProvider, Provider};

fn drop_deploy<P: AsMidnightProvider + Provider>(provider: P) {
    Contract::deploy(provider);
}

fn drop_connect<P: AsMidnightProvider + Provider>(provider: P) {
    Contract::at(provider, "00");
}

fn drop_call<P: AsMidnightProvider + Provider>(contract: &Contract<P>) {
    contract.circuits().increment();
}

fn main() {}
