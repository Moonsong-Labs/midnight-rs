//! The places where the ledger generations disagree.
//!
//! Upstream already hides most of the difference: `ledger_8` and `ledger_9`
//! compile one shared source against different crates, and each adds a small
//! set of functions (`contract_operation_new`, `transaction_signature`, ...)
//! with matching names and different bodies. Those cover construction: a plain
//! value in, the generation's own type out.
//!
//! This module covers the rest, which is mostly the other direction. Reading a
//! generation's type back into a plain value needs a `match` on ledger 9 and
//! nothing at all on ledger 8, so a caller that writes either one stops
//! compiling on the other. Every such place lives here.
//!
//! Keep the surface small. A new entry belongs here only when the two
//! generations need different code, never when they merely need different
//! types: the aliases at the crate root already carry those.

use midnight_base_crypto::signatures::{Signature, VerifyingKey};

use crate::TransactionSignature;
use crate::onchain_runtime::state::ContractOperation;
use crate::{ContractOperationVersion, ContractOperationVersionedVerifierKey};

/// A member of a contract's on-chain maintenance committee.
///
/// Ledger 9 tags each member with its signature kind. Earlier generations hold
/// the Schnorr key directly.
#[cfg(not(feature = "ledger-9"))]
pub type CommitteeKey = crate::SignatureVerifyingKey;
#[cfg(feature = "ledger-9")]
pub type CommitteeKey = crate::ContractMaintenanceVerifyingKey;

/// The Schnorr signature inside `signature`, or `None` when it is another kind.
///
/// This workspace signs and verifies with Schnorr keys only. Report the other
/// kinds rather than treat them as a Schnorr key that fails to verify.
#[cfg(not(feature = "ledger-9"))]
pub fn schnorr_signature(signature: &TransactionSignature) -> Option<&Signature> {
    Some(signature)
}
#[cfg(feature = "ledger-9")]
pub fn schnorr_signature(signature: &TransactionSignature) -> Option<&Signature> {
    match signature {
        TransactionSignature::Schnorr(signature) => Some(signature),
        _ => None,
    }
}

/// The Schnorr key inside `key`, or `None` when it is another kind.
///
/// A committee holding a kind this workspace cannot sign with is reported, not
/// skipped: dropping a member would mis-count the threshold.
#[cfg(not(feature = "ledger-9"))]
pub fn schnorr_committee_key(key: &CommitteeKey) -> Option<&VerifyingKey> {
    Some(key)
}
#[cfg(feature = "ledger-9")]
pub fn schnorr_committee_key(key: &CommitteeKey) -> Option<&VerifyingKey> {
    match key {
        CommitteeKey::Schnorr(key) => Some(key),
        _ => None,
    }
}

/// Whether `op` carries a verifier key in any slot.
#[cfg(not(feature = "ledger-9"))]
pub fn has_verifier_key(op: &ContractOperation) -> bool {
    op.v2.is_some()
}
#[cfg(feature = "ledger-9")]
pub fn has_verifier_key(op: &ContractOperation) -> bool {
    op.v2_vk().is_some() || op.v3_vk().is_some()
}

/// `op`'s key, wrapped in the maintenance-update enum for the slot it sits in.
///
/// Returns `None` when `op` carries no key. Each slot holds a key of its own
/// type, so the wrapping and the slot are one decision.
#[cfg(not(feature = "ledger-9"))]
pub fn versioned_verifier_key(
    op: ContractOperation,
) -> Option<ContractOperationVersionedVerifierKey> {
    op.v2.map(ContractOperationVersionedVerifierKey::V3)
}
#[cfg(feature = "ledger-9")]
pub fn versioned_verifier_key(
    op: ContractOperation,
) -> Option<ContractOperationVersionedVerifierKey> {
    match (op.v2, op.v3) {
        (Some(vk), _) => Some(ContractOperationVersionedVerifierKey::V3(vk)),
        (_, Some(vk)) => Some(ContractOperationVersionedVerifierKey::V4(vk)),
        _ => None,
    }
}

/// The slot `op`'s key occupies.
///
/// `VerifierKeyRemove` names a slot rather than a key, so a removal has to
/// agree with the insert that filled it. Ledger 9 has two slots and picks by
/// the key's own tag, so the answer has to be read from the operation.
#[cfg(not(feature = "ledger-9"))]
pub fn key_slot(_op: &ContractOperation) -> ContractOperationVersion {
    ContractOperationVersion::V3
}
#[cfg(feature = "ledger-9")]
pub fn key_slot(op: &ContractOperation) -> ContractOperationVersion {
    if op.v3_vk().is_some() {
        ContractOperationVersion::V4
    } else {
        ContractOperationVersion::V3
    }
}

/// The slot a versioned key belongs in, for an insert this build just made.
#[cfg(not(feature = "ledger-9"))]
pub fn versioned_key_slot(_vk: &ContractOperationVersionedVerifierKey) -> ContractOperationVersion {
    ContractOperationVersion::V3
}
#[cfg(feature = "ledger-9")]
pub fn versioned_key_slot(vk: &ContractOperationVersionedVerifierKey) -> ContractOperationVersion {
    match vk {
        ContractOperationVersionedVerifierKey::V4(_) => ContractOperationVersion::V4,
        _ => ContractOperationVersion::V3,
    }
}

/// Whether this generation can store a verifier key carrying `tag`.
///
/// `contract_operation_new` panics on a tag it does not know, so check first
/// and report the tag instead.
#[cfg(not(feature = "ledger-9"))]
pub fn accepts_verifier_key_tag(tag: &str) -> bool {
    tag == "verifier-key[v6]"
}
#[cfg(feature = "ledger-9")]
pub fn accepts_verifier_key_tag(tag: &str) -> bool {
    tag == "verifier-key[v6]" || tag == "verifier-key[v7]"
}

/// Whether this generation stores a circuit's IR on chain beside its verifier key.
#[cfg(not(feature = "ledger-9"))]
pub const STORES_IR_ON_CHAIN: bool = false;
#[cfg(feature = "ledger-9")]
pub const STORES_IR_ON_CHAIN: bool = true;

/// Emit `ProvingProvider::resolver` where the ledger's trait declares it.
///
/// Call this inside an `impl ProvingProvider`, passing the expression that
/// yields the resolver. A dependent cannot decide this with its own `#[cfg]`:
/// its feature and this crate's can differ whenever something else in the
/// graph turns this crate's on, and then the impl and the trait disagree.
#[cfg(feature = "ledger-9")]
#[macro_export]
macro_rules! proving_provider_resolver {
    ($self:ident -> $body:expr) => {
        fn resolver(&$self) -> &impl $crate::transient_crypto::proofs::Resolver {
            $body
        }
    };
}
#[cfg(not(feature = "ledger-9"))]
#[macro_export]
macro_rules! proving_provider_resolver {
    ($self:ident -> $body:expr) => {};
}

/// Whether `op` carries the circuit's IR.
///
/// Always false where the generation has no slot for it, so pair this with
/// [`STORES_IR_ON_CHAIN`] rather than asserting on it directly.
#[cfg(not(feature = "ledger-9"))]
pub fn has_ir(_op: &ContractOperation) -> bool {
    false
}
#[cfg(feature = "ledger-9")]
pub fn has_ir(op: &ContractOperation) -> bool {
    op.ir.is_some()
}
