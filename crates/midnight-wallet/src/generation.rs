//! Which ledger generation an indexer value is in, read from its tag.

use std::io::Cursor;

use midnight_helpers::midnight_serialize::peek_tag;
use midnight_helpers::{DefaultDB, Tagged, ledger_8};
use serde::{Deserialize, Serialize};

use crate::WalletError;

/// A generation of the Midnight ledger.
///
/// The indexer serves the events of ledger 8 and ledger 9 in one stream, so a
/// replay reads each event's generation from its tag. The serde names are the
/// `ledger_version` that a snapshot's `metadata.json` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Generation {
    /// Ledger 8, which a chain ran until its hard fork to ledger 9.
    V8,
    /// Ledger 9.
    V9,
}

impl Generation {
    /// The generation of an encoded ledger event, as the indexer's
    /// `zswapLedgerEvents` and `dustLedgerEvents` serve it.
    ///
    /// # Errors
    ///
    /// [`WalletError::UnknownLedger`] when the bytes are no event of ledger 8
    /// or ledger 9.
    pub(crate) fn of_event(bytes: &[u8]) -> Result<Self, WalletError> {
        let tag = tag_of(bytes)?;
        if tag == ledger_8::Event::<DefaultDB>::tag() {
            Ok(Self::V8)
        } else if tag == midnight_helpers::Event::<DefaultDB>::tag() {
            Ok(Self::V9)
        } else {
            Err(WalletError::UnknownLedger { tag })
        }
    }
}

/// Refuse encoded ledger parameters of any generation but ledger 9.
///
/// The indexer serves a block's parameters in the generation that the block
/// runs, so parameters of another generation name a chain this build does
/// not sync, such as one that runs ledger 8.
///
/// # Errors
///
/// [`WalletError::UnknownLedger`] that names the tag of the parameters.
pub(crate) fn require_ledger_9_parameters(bytes: &[u8]) -> Result<(), WalletError> {
    let tag = tag_of(bytes)?;
    if tag == midnight_helpers::LedgerParameters::tag() {
        Ok(())
    } else {
        Err(WalletError::UnknownLedger { tag })
    }
}

fn tag_of(bytes: &[u8]) -> Result<String, WalletError> {
    peek_tag(&mut Cursor::new(bytes))
        .map(|tag| tag.to_string())
        .map_err(|e| WalletError::Sync(format!("read the tag of ledger data: {e}")))
}

#[cfg(test)]
mod tests {
    use midnight_helpers::Serializable;
    use midnight_helpers::midnight_serialize::tagged_serialize;

    use super::*;

    fn encoded<T: Serializable + Tagged>(value: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        tagged_serialize(value, &mut bytes).unwrap();
        bytes
    }

    /// A marker that lost the ledger 8 event tag would fail every replay
    /// from event 0 on a chain that crossed the fork. One that swapped the
    /// tags would decode each event with the other generation's types, which
    /// fails far from here with an opaque error.
    #[test]
    fn each_generation_owns_its_own_encoding() {
        assert_eq!(
            Generation::of_event(b"midnight:event[v9]:").unwrap(),
            Generation::V8
        );
        assert_eq!(
            Generation::of_event(b"midnight:event[v14]:").unwrap(),
            Generation::V9
        );

        require_ledger_9_parameters(&encoded(&midnight_helpers::INITIAL_PARAMETERS)).unwrap();
        let ledger_8_chain =
            require_ledger_9_parameters(b"midnight:ledger-parameters[v5]:").unwrap_err();
        assert!(
            matches!(&ledger_8_chain, WalletError::UnknownLedger { tag } if tag == "ledger-parameters[v5]"),
            "{ledger_8_chain:?}"
        );
    }
}
