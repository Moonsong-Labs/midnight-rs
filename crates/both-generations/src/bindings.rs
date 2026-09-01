//! The same contract, bound to both ledger generations, in one crate.
//!
//! `contract!` takes the crate its generated code imports from, so pointing it
//! at a per-generation shim yields typed bindings for that generation. Two
//! invocations give both, and a dispatching wrapper over them would need no
//! change to `midnight-contract` at all.

/// Counter bindings against ledger 8.
pub mod ledger_8 {
    compact_bindgen_v8::contract!(
        #[crate(compact_bindgen_v8)]
        "../../devnet/contracts/counter/compiled/analyzed-ir.sexp"
    );
}

/// Counter bindings against ledger 9.
pub mod ledger_9 {
    compact_bindgen_v9::contract!(
        #[crate(compact_bindgen_v9)]
        "../../devnet/contracts/counter/compiled/analyzed-ir.sexp"
    );
}

/// A counter's ledger state, on whichever generation the chain runs.
///
/// This is what `contract!` would emit around the two binding sets above. The
/// generated accessors return plain Rust types, so the wrapper forwards them
/// and the caller never names a generation.
pub enum Counter {
    /// State read from a ledger 8 chain.
    Ledger8(ledger_8::Ledger),
    /// State read from a ledger 9 chain.
    Ledger9(ledger_9::Ledger),
}

impl Counter {
    /// Parse hex-encoded state, as the indexer serves it, for `generation`.
    pub fn from_hex(
        generation: midnight_dispatch::Generation,
        hex_state: &str,
    ) -> Result<Self, String> {
        match generation {
            midnight_dispatch::Generation::Ledger8 => ledger_8::Ledger::from_hex(hex_state)
                .map(Counter::Ledger8)
                .map_err(|e| e.to_string()),
            midnight_dispatch::Generation::Ledger9 => ledger_9::Ledger::from_hex(hex_state)
                .map(Counter::Ledger9)
                .map_err(|e| e.to_string()),
        }
    }

    /// The counter's `round` field.
    pub fn round(&self) -> Result<u64, String> {
        match self {
            Counter::Ledger8(ledger) => ledger.round().map_err(|e| e.to_string()),
            Counter::Ledger9(ledger) => ledger.round().map_err(|e| e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    /// The two binding sets are typed against different generations, so the
    /// same contract yields two distinct `Ledger` types in one crate. A
    /// dispatching wrapper over these needs no change to `midnight-contract`.
    #[test]
    fn one_contract_yields_bindings_for_both_generations() {
        let eight = super::ledger_8::LedgerInitialState::default().build();
        let nine = super::ledger_9::LedgerInitialState::default().build();
        // Distinct types: each `build()` returns its own generation's
        // `InitialState`, so this only compiles because both are linked.
        assert!(
            std::any::type_name_of_val(&eight).contains("v8"),
            "ledger 8 bindings should carry the v8 contract crate"
        );
        assert!(
            std::any::type_name_of_val(&nine).contains("v9"),
            "ledger 9 bindings should carry the v9 contract crate"
        );
        eprintln!(
            "8: {}\n9: {}",
            std::any::type_name_of_val(&eight),
            std::any::type_name_of_val(&nine)
        );
    }
}
