//! The same contract, bound to both ledger generations, from one macro call.
//!
//! `#[dispatch]` emits a binding set per generation plus a wrapper over them,
//! so a caller reads a contract without naming the generation the chain runs.

compact_bindgen_v9::contract!(
    #[dispatch]
    Counter,
    "../../devnet/contracts/counter/compiled/analyzed-ir.sexp"
);

#[cfg(test)]
mod tests {
    /// The two binding sets are typed against different generations, so the
    /// same contract yields two distinct `InitialState` types in one crate.
    #[test]
    fn one_contract_yields_bindings_for_both_generations() {
        let eight = super::counter::ledger_8::CounterInitialState::default().build();
        let nine = super::counter::ledger_9::CounterInitialState::default().build();
        assert!(
            std::any::type_name_of_val(&eight).contains("v8"),
            "ledger 8 bindings should carry the v8 contract crate"
        );
        assert!(
            std::any::type_name_of_val(&nine).contains("v9"),
            "ledger 9 bindings should carry the v9 contract crate"
        );
    }
}

// A contract with map and set fields, to check the wrapper forwards those too.
compact_bindgen_v9::contract!(
    #[dispatch]
    Gateway,
    "../../tests/fixtures/compiled/gateway/compiler/analyzed-ir.sexp"
);

/// A collection field's size and emptiness read the same on either generation,
/// so the wrapper forwards them. Reading an entry returns a per-generation
/// reader and is not forwarded, so this names only the two that are.
///
/// Referencing them is the assertion: the wrapper stops compiling if the
/// generator drops them.
const _: fn(&gateway::GatewayDispatch) = |gateway| {
    let _ = gateway.egress_jobs_size();
    let _ = gateway.egress_jobs_is_empty();
};

// A contract whose map holds primitives, so reading an entry forwards.
compact_bindgen_v9::contract!(
    #[dispatch]
    Containers,
    "../../tests/conformance/fixtures/containers/compiler/analyzed-ir.sexp"
);

/// A map of primitives forwards `get` as well as its size, because both key and
/// value are the same type in either binding set. Referencing it is the
/// assertion: the wrapper stops compiling if the generator drops it.
const _: fn(&containers::ContainersDispatch) = |containers| {
    let _: Result<Option<u64>, String> = containers.scores_get(7u8);
    let _ = containers.scores_size();
};
