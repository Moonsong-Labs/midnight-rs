//! Names for the error codes a node returns when it refuses a transaction.
//!
//! A refusal arrives as substrate's `InvalidTransaction::Custom(u8)`, rendered
//! into prose by the RPC layer.
//!
//! ## Why the names are here and not fetched
//!
//! Nothing serves this mapping, which was checked rather than assumed:
//!
//! - The node exposes no RPC for it. Of the 138 methods it lists, the
//!   `midnight_` ones are `apiVersions`, `contractState`, `ledgerStateRoot`,
//!   `ledgerVersion` and `zswapStateRoot`.
//! - Metadata carries the error *types*, so `FeeCalculation` and
//!   `DustDoubleSpend` do appear in the blob, but not the codes. The byte comes
//!   from a hand-written `From<LedgerApiError> for u8` in the node, which
//!   flattens a nested enum into reserved ranges. `LedgerApiError` has 11
//!   top-level variants and the flattening spans about 119 codes, so no
//!   variant index is the code: `InputNotInUtxos` is 195 and sits at index 11
//!   of its own enum.
//! - Substrate reserves `Custom(u8)` for the chain to define, so it is opaque
//!   by design.
//!
//! Generating the reverse in midnight-node and depending on it would beat
//! either, and it does not work yet. The crate that owns the mapping,
//! `midnight-node-ledger`, pulls `frame-support`, `sp-runtime` and several
//! ledger generations, which this SDK does not depend on. More to the point,
//! the pinned fork in `Cargo.toml` and the node image in
//! `devnet/docker-compose.yml` move independently, and today they disagree on
//! 52 codes: 168 has no meaning in the fork while the image we run returns it.
//! A generated table would be right for the fork and wrong for the node.
//!
//! ## So this names only what we have seen
//!
//! Each entry below came back from a running node and is recorded where it was
//! observed. Transcribing the node's whole table instead was tried and
//! rejected: flattening its nested enums drops the qualifier, and ten names
//! then collide. Codes 5 and 59 are both `VersionedArenaKey`, one for
//! deserialization and one for serialization, and codes 103 and 127 are both
//! `Zswap`, one an `InvalidError` and one a `MalformedError`. A name that can
//! mean two things is worse than a bare number.
//!
//! A code with no entry renders as the number the node sent, which is what a
//! caller had before this module existed. Adding one takes two files in
//! midnight-node, at the tag matching the image in
//! `devnet/docker-compose.yml`. `ledger/src/versions/common/types.rs` holds
//! the numbers, in `From<LedgerApiError> for u8`. Beside it,
//! `ledger/src/versions/common/conversions.rs` maps each ledger failure onto
//! the node enum variant that number names, which is where a qualifier like
//! `InvalidError` or `MalformedError` comes from.
//!
//! Codes are not stable across node releases, which was measured rather than
//! feared. Between the tag we pin and a midnight-node revision four months
//! later, the table grows from 119 assignments to 154, three codes change
//! meaning, and 168 is withdrawn entirely. Treat a name as a reading aid,
//! never as a protocol constant.

/// `(code, name)`, sorted by code. Qualified as the node writes them, so a
/// name says which enum it came from.
static CODES: &[(u8, &str)] = &[
    // Registering with two unshielded inputs, which puts the transaction
    // outside its time to dismiss (`dust_registration_submit`).
    //
    // This one is version-bound, and it is the reason the module says a name
    // is a reading aid. The node we pin emits it; a later midnight-node
    // retires the assignment and gives 168 no meaning at all. The other four
    // entries are unchanged across the same span.
    (168, "MalformedError::FeeCalculation"),
    // Transferring a pre-allocated dev token with chain-side restrictions
    // (`midnight-wallet` integration tests).
    (171, "MalformedError::OutOfDustValidityWindow"),
    // Building a registration whose declared allowance the guaranteed offer
    // did not back (`dust_registration_offer`).
    (173, "MalformedError::InsufficientDustForRegistrationFee"),
    // A second build re-selecting an input the first already spent.
    (195, "InvalidError::InputNotInUtxos"),
    // Two wallets on one seed drawing the same Dust note.
    (196, "InvalidError::DustDoubleSpend"),
];

/// The name midnight-node gives `code`, or `None` when this build has never
/// seen it. An unnamed code is ordinary: the caller still has the number and
/// the node's own message.
pub fn name_of(code: u8) -> Option<&'static str> {
    CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
}

/// The `Custom(u8)` code carried by a node's refusal message, when it has one.
///
/// Read out of the text because that is the only form available: subxt's
/// `TransactionStatus::Invalid` carries a human-readable `message` and nothing
/// else, so the watch stream has already discarded the typed
/// `InvalidTransaction::Custom(u8)` by the time we see it. There is no typed
/// value on this path to reach for instead.
///
/// A message that does not carry one yields `None`, which is not an error:
/// plenty of refusals (a bad nonce, a stale mortality) never reach the
/// ledger's own error mapping.
pub fn code_in(message: &str) -> Option<u8> {
    let tail = message.rsplit_once("custom error")?.1;
    let digits: String = tail
        .trim_start_matches([':', ' '])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_has_no_repeats() {
        for pair in CODES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "codes must be sorted and distinct: {} then {}",
                pair[0].0,
                pair[1].0
            );
        }
    }

    /// A name has to say which enum it came from. The node reuses bare names
    /// across its nested error types, so an unqualified one can mean two
    /// different failures.
    #[test]
    fn every_name_is_qualified_and_distinct() {
        for (code, name) in CODES {
            assert!(
                name.contains("::"),
                "code {code} is named {name}, which does not say which enum it came from"
            );
        }
        for (i, (_, name)) in CODES.iter().enumerate() {
            assert!(
                !CODES[i + 1..].iter().any(|(_, other)| other == name),
                "{name} names more than one code"
            );
        }
    }

    #[test]
    fn codes_this_repo_has_observed_keep_their_names() {
        assert_eq!(name_of(168), Some("MalformedError::FeeCalculation"));
        assert_eq!(
            name_of(171),
            Some("MalformedError::OutOfDustValidityWindow")
        );
        assert_eq!(
            name_of(173),
            Some("MalformedError::InsufficientDustForRegistrationFee")
        );
        assert_eq!(name_of(195), Some("InvalidError::InputNotInUtxos"));
        assert_eq!(name_of(196), Some("InvalidError::DustDoubleSpend"));
    }

    #[test]
    fn a_code_we_have_not_seen_is_left_as_a_number() {
        assert_eq!(name_of(42), None);
    }

    #[test]
    fn a_code_outside_a_byte_is_not_a_custom_code() {
        // `InvalidTransaction::Custom` carries a u8, so anything wider came
        // from somewhere else and must not be named as if it had.
        assert_eq!(code_in("custom error: 300"), None);
        assert_eq!(code_in("custom error: 255"), Some(255));
    }

    #[test]
    fn only_digits_are_taken_and_a_missing_number_is_not_one() {
        assert_eq!(code_in("custom error: 168, and more text"), Some(168));
        assert_eq!(code_in("custom error: -5"), None);
        assert_eq!(code_in("custom error:"), None);
        // The last occurrence wins, which is the one the node appended.
        assert_eq!(code_in("custom error: 1 then custom error: 173"), Some(173));
    }

    #[test]
    fn a_code_is_read_out_of_the_node_message() {
        assert_eq!(
            code_in("Invalid transaction with custom error: 168"),
            Some(168)
        );
        assert_eq!(
            code_in("Invalid transaction with custom error:173"),
            Some(173)
        );
        assert_eq!(code_in("Transaction has a bad signature"), None);
        assert_eq!(code_in("custom error: not-a-number"), None);
    }
}
