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
