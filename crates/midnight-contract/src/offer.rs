//! A hand-built shielded offer for a deploy.

use midnight_types::LedgerVersion;

/// A hand-built shielded (Zswap) offer to ride alongside a deploy, built with
/// the types of the chain's ledger generation.
pub enum ShieldedOffer {
    /// An offer for a chain on ledger 8, from the types in
    /// [`crate::ledger_8`].
    Ledger8(
        midnight_helpers::ledger_8::OfferInfo<
            midnight_helpers::DefaultDB,
            midnight_helpers::ledger_8::BuildContext,
        >,
    ),
    /// An offer for a chain on ledger 9, from the types in
    /// [`crate::ledger_9`].
    Ledger9(
        midnight_helpers::ledger_9::OfferInfo<
            midnight_helpers::DefaultDB,
            midnight_helpers::ledger_9::BuildContext,
        >,
    ),
}

impl std::fmt::Debug for ShieldedOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ledger = match self {
            Self::Ledger8(_) => LedgerVersion::V8,
            Self::Ledger9(_) => LedgerVersion::V9,
        };
        f.debug_struct("ShieldedOffer")
            .field("ledger", &ledger)
            .finish_non_exhaustive()
    }
}
