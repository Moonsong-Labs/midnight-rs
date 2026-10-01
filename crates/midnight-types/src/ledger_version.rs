//! Which generation of the Midnight ledger a chain runs.

use std::fmt;
use std::io::Cursor;

use midnight_helpers::midnight_serialize::peek_tag;
use midnight_helpers::{Tagged, ledger_8, ledger_9};
use serde::{Deserialize, Serialize};

/// A generation of the Midnight ledger: the transaction format and the rules
/// a chain applies to it.
///
/// A chain changes generation only at a hard fork. The SDK reads the
/// generation from the chain's data, never from configuration: from the tag
/// of a value whose encoding changed at the fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LedgerVersion {
    /// Ledger 8, which mainnet, preprod and preview run until their hard fork.
    V8,
    /// Ledger 9, which a chain runs from its 8 to 9 hard fork on.
    V9,
}

impl LedgerVersion {
    /// The generation of an encoded ledger event, as the indexer's
    /// `zswapLedgerEvents` and `dustLedgerEvents` serve them.
    ///
    /// # Errors
    ///
    /// When the bytes are no event of a linked generation.
    pub fn of_event(bytes: &[u8]) -> Result<Self, UnknownLedger> {
        of_tagged::<ledger_8::Event<ledger_8::DefaultDB>, ledger_9::Event<ledger_9::DefaultDB>>(
            bytes,
        )
    }

    /// The generation of encoded ledger parameters, as the indexer serves
    /// them on every block. A block's parameters are in the generation that
    /// block runs.
    ///
    /// # Errors
    ///
    /// When the bytes are no parameters of a linked generation.
    pub fn of_ledger_parameters(bytes: &[u8]) -> Result<Self, UnknownLedger> {
        of_tagged::<ledger_8::LedgerParameters, ledger_9::LedgerParameters>(bytes)
    }

    /// The generation a block ran, from the ledger parameters the indexer
    /// serves on it.
    ///
    /// # Errors
    ///
    /// When the block carries no ledger parameters, or parameters of no
    /// linked generation.
    pub fn of_block(block: &midnight_indexer_client::Block) -> Result<Self, UnknownLedger> {
        let hex = block
            .ledger_parameters
            .as_deref()
            .ok_or_else(|| UnknownLedger("a block without ledger parameters".into()))?;
        let bytes = hex::decode(hex)
            .map_err(|e| UnknownLedger(format!("ledger parameters that are not hex ({e})")))?;
        Self::of_ledger_parameters(&bytes)
    }

    /// The generation of an encoded transaction.
    ///
    /// # Errors
    ///
    /// When the bytes are no transaction of a linked generation.
    pub fn of_transaction(bytes: &[u8]) -> Result<Self, UnknownLedger> {
        of_tagged::<
            ledger_8::FinalizedTransaction<ledger_8::DefaultDB>,
            ledger_9::FinalizedTransaction<ledger_9::DefaultDB>,
        >(bytes)
    }
}

/// The generation whose encoding of one value these tagged bytes carry, `T8`
/// for ledger 8 and `T9` for ledger 9. The two must carry different tags.
fn of_tagged<T8: Tagged, T9: Tagged>(bytes: &[u8]) -> Result<LedgerVersion, UnknownLedger> {
    let tag = peek_tag(&mut Cursor::new(bytes)).map_err(|e| UnknownLedger(e.to_string()))?;
    if tag == T8::tag() {
        Ok(LedgerVersion::V8)
    } else if tag == T9::tag() {
        Ok(LedgerVersion::V9)
    } else {
        Err(UnknownLedger(format!("`{tag}`")))
    }
}

impl fmt::Display for LedgerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V8 => f.write_str("ledger 8"),
            Self::V9 => f.write_str("ledger 9"),
        }
    }
}

/// Encoded ledger data of no generation this build links: a chain that runs a
/// later ledger, or bytes that are not ledger data.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0} is not ledger 8 or ledger 9 data")]
pub struct UnknownLedger(String);

#[cfg(test)]
mod tests {
    use midnight_helpers::midnight_serialize::tagged_serialize;

    use super::*;

    fn encoded<T: midnight_helpers::Serializable + Tagged>(value: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        tagged_serialize(value, &mut bytes).unwrap();
        bytes
    }

    /// A swapped or one-sided comparison would decode every chain with the
    /// other generation's types, which fails far from here with an opaque
    /// decode error.
    #[test]
    fn each_generation_owns_its_own_encoding() {
        let of = LedgerVersion::of_ledger_parameters;

        assert_eq!(
            of(&encoded(&ledger_8::INITIAL_PARAMETERS)).unwrap(),
            LedgerVersion::V8
        );
        assert_eq!(
            of(&encoded(&ledger_9::INITIAL_PARAMETERS)).unwrap(),
            LedgerVersion::V9
        );
        let other = of(&encoded(&ledger_9::INITIAL_PARAMETERS.dust)).unwrap_err();
        assert!(other.to_string().contains("dust-parameters"), "{other}");
    }
}
