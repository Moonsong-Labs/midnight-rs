use crate::H256;

/// The hash of a block. Format it with `{:x}`: its `Display` abbreviates.
pub type BlockHash = H256;

/// The hash of an extrinsic, the node's identity for a submission. Format it
/// with `{:x}`: its `Display` abbreviates.
pub type ExtrinsicHash = H256;
