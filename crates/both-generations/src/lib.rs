//! Both ledger generations, linked into one crate.
//!
//! Cargo does not unify features across distinct packages, so the two shims
//! over `midnight-types` select different generations and coexist here. The
//! transparent dispatch layer needs that property, and this crate is what
//! keeps it true.

pub mod bindings;

/// The transaction serialization tag each generation writes.
///
/// The tags differ, which is the whole reason a client cannot speak to a
/// chain on the other generation.
pub fn transaction_tags() -> (&'static str, &'static str) {
    (
        <v8::Transaction<
            v8::Signature,
            v8::ProofMarker,
            v8::PedersenRandomness,
            v8::DefaultDB,
        > as v8::midnight_serialize::Tagged>::tag()
        .into_owned()
        .leak(),
        <v9::Transaction<
            v9::Signature,
            v9::ProofMarker,
            v9::PedersenRandomness,
            v9::DefaultDB,
        > as v9::midnight_serialize::Tagged>::tag()
        .into_owned()
        .leak(),
    )
}

/// The verifier-key tags each generation accepts.
///
/// Ledger 9 gained a second slot, so it takes a key ledger 8 refuses. The
/// contract stacks above these facades are linked twice as well, which is
/// what the dependencies of this crate hold.
pub fn accepted_verifier_key_tags() -> (bool, bool) {
    (
        v8::compat::accepts_verifier_key_tag("verifier-key[v7]"),
        v9::compat::accepts_verifier_key_tag("verifier-key[v7]"),
    )
}

/// Both contract stacks, linked. Naming a type from each keeps the linker
/// honest: drop this and the dependency becomes decorative.
pub fn contract_stacks_link() -> (usize, usize) {
    (
        std::mem::size_of::<c8::InitialState>(),
        std::mem::size_of::<c9::InitialState>(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_binary_holds_both_generations() {
        let (eight, nine) = super::transaction_tags();
        assert_ne!(
            eight, nine,
            "the generations must differ, or this proves nothing"
        );
        eprintln!("ledger 8: {eight}\nledger 9: {nine}");

        let (v7_on_8, v7_on_9) = super::accepted_verifier_key_tags();
        assert!(!v7_on_8, "ledger 8 has one verifier-key slot");
        assert!(v7_on_9, "ledger 9 has two");
    }
}
