//! Telling one chain from another, so a resume cannot serve a dead chain's
//! balance.
//!
//! A persisted wallet keeps event cursors, and cursors are counts. Recreate
//! the chain under it and those counts still look plausible: a resume asks
//! for events past the end of the fresh chain, the indexer sends none, and
//! the replay reads that silence as "already at tip". Nothing in the cursors
//! says which chain produced them.
//!
//! So the snapshot carries a [`ChainPin`], a finalized block it saw. Finality
//! is what makes the pin worth checking. A finalized block cannot be reorged
//! away, so a height that no longer holds it means the chain was replaced,
//! not that a fork won. That reasoning holds on any network, which is why
//! this needs no "only on a dev chain" gate.
//!
//! The node answers the question. The pin is compared against what the node
//! reports for that height, never against the indexer, which is a projection
//! of the chain rather than the chain.

use serde::{Deserialize, Serialize};

/// A finalized block a wallet snapshot saw, kept so a later resume can ask
/// whether it is still looking at the same chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainPin {
    pub height: i64,
    /// The block hash as the node renders it, `0x` prefixed.
    pub hash: String,
}

/// What a pin says about the chain in front of us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainCheck {
    /// The node still has that block. The cached cursors belong to this chain.
    SameChain,
    /// The chain no longer holds the pinned block, so the cursors describe a
    /// chain that is gone.
    Replaced { found: Option<String> },
    /// The node could not answer. Says nothing either way, so the caller
    /// carries on: a pruned or partial archive must not condemn a healthy
    /// wallet.
    Unknown,
}

/// Judge a pin against the hashes a node reports for its height.
///
/// `hashes` is `None` when the node could not answer at all. An empty slice
/// is an answer: the chain has no block at that height, so it is shorter than
/// the one the snapshot saw.
pub fn check_chain_pin(pin: &ChainPin, hashes: Option<&[String]>) -> ChainCheck {
    let Some(hashes) = hashes else {
        return ChainCheck::Unknown;
    };
    if hashes.iter().any(|h| h.eq_ignore_ascii_case(&pin.hash)) {
        return ChainCheck::SameChain;
    }
    ChainCheck::Replaced {
        found: hashes.first().cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> ChainPin {
        ChainPin {
            height: 42,
            hash: "0xabc".to_string(),
        }
    }

    /// The message is the whole recovery instruction, so it has to name the
    /// directory rather than describe it.
    #[test]
    fn the_mismatch_error_tells_the_reader_what_to_remove() {
        let err = crate::WalletError::ChainMismatch {
            path: "/home/me/.midnight/wallets/undeployed/abc".to_string(),
            pinned_height: 633,
            pinned_hash: "0xd886".to_string(),
            found: "no block".to_string(),
        };
        let text = err.to_string();
        assert!(
            text.contains("/home/me/.midnight/wallets/undeployed/abc"),
            "{text}"
        );
        assert!(text.contains("633"), "{text}");
        assert!(text.contains("no block"), "{text}");
    }

    #[test]
    fn the_same_block_at_that_height_is_the_same_chain() {
        let hashes = ["0xabc".to_string()];
        assert_eq!(
            check_chain_pin(&pin(), Some(&hashes)),
            ChainCheck::SameChain
        );
    }

    #[test]
    fn a_different_block_at_that_height_means_the_chain_was_replaced() {
        let hashes = ["0xdef".to_string()];
        assert_eq!(
            check_chain_pin(&pin(), Some(&hashes)),
            ChainCheck::Replaced {
                found: Some("0xdef".to_string())
            }
        );
    }

    /// A fresh chain that has not yet climbed to the pinned height reports no
    /// block there, which is still an answer.
    #[test]
    fn no_block_at_that_height_means_the_chain_was_replaced() {
        assert_eq!(
            check_chain_pin(&pin(), Some(&[])),
            ChainCheck::Replaced { found: None }
        );
    }

    #[test]
    fn a_node_that_cannot_answer_condemns_nothing() {
        assert_eq!(check_chain_pin(&pin(), None), ChainCheck::Unknown);
    }

    /// An unfinalized fork can leave several hashes at one height. The pin is
    /// finalized, so finding it among them is enough.
    #[test]
    fn the_pin_is_found_among_several_hashes_at_one_height() {
        let hashes = ["0xdef".to_string(), "0xabc".to_string()];
        assert_eq!(
            check_chain_pin(&pin(), Some(&hashes)),
            ChainCheck::SameChain
        );
    }

    #[test]
    fn hash_casing_does_not_decide_it() {
        let hashes = ["0xABC".to_string()];
        assert_eq!(
            check_chain_pin(&pin(), Some(&hashes)),
            ChainCheck::SameChain
        );
    }
}
