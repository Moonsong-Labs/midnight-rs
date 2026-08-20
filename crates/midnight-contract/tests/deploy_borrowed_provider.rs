//! A deploy must be awaitable on a borrowed provider.
//!
//! `DeployBuilder`'s `IntoFuture` boxes the deploy, and a box with no lifetime
//! of its own defaults to `'static`. That default is what rules the borrow out,
//! so this only has to compile.

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/analyzed-ir.sexp");
}

use std::future::IntoFuture;

use midnight_contract::Contract;
use midnight_provider::MidnightProvider;

#[test]
fn a_borrowed_provider_reaches_into_future() {
    let provider = MidnightProvider::new("ws://test", "http://test").unwrap();

    drop(
        Contract::deploy(&provider)
            .with_zk_config("compiled")
            .into_future(),
    );

    drop(
        counter::Contract::deploy(&provider)
            .with_initial_state(counter::LedgerInitialState::default())
            .with_zk_config("compiled")
            .into_future(),
    );
}

#[test]
fn an_owned_provider_still_reaches_into_future() {
    let provider = MidnightProvider::new("ws://test", "http://test").unwrap();

    drop(
        counter::Contract::deploy(provider)
            .with_initial_state(counter::LedgerInitialState::default())
            .with_zk_config("compiled")
            .into_future(),
    );
}
