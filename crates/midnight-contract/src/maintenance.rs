//! Contract maintenance / governance.
//!
//! A contract's on-chain `maintenance_authority` is a k-of-n committee that
//! controls verifier-key rotation and authority replacement. This SDK does not
//! hold any signing key: you set the committee (verifying keys) at deploy, and
//! every maintenance op is signed externally — you get the bytes to sign, the
//! committee members sign them with their own keys, and you submit the
//! transaction with the collected signatures. See
//! `docs/contract-maintenance-governance.md` for the protocol model.

use midnight_onchain_runtime::state::{ContractMaintenanceVerifyingKey, EntryPointBuf};
use midnight_provider::PendingTx;
use midnight_serialize::{Deserializable, Serializable};
use midnight_typed_state::{ContractMaintenanceAuthority, ContractState, InMemoryDB};
use midnight_types::{MidnightWallet, TxEnvelope};

use crate::contract::{AsMidnightProvider, Contract};
use crate::error::ContractError;
use crate::{ContractMaintenanceSignature, ContractMaintenanceSigningKey, ecdsa};

// ---------------------------------------------------------------------------
// Deploy-side: stamp a committee into the contract state.
// ---------------------------------------------------------------------------

/// Set the contract's maintenance authority to `committee` with the given
/// `threshold`, `counter = 0`.
///
/// This is what makes a freshly-built deploy state governable. The default
/// authority produced by codegen has an empty committee, which can never
/// authorize a maintenance update.
pub(crate) fn set_maintenance_authority(
    mut state: ContractState<InMemoryDB>,
    committee: Vec<ContractMaintenanceVerifyingKey>,
    threshold: u32,
) -> ContractState<InMemoryDB> {
    state.maintenance_authority = ContractMaintenanceAuthority {
        committee,
        threshold,
        counter: 0,
    };
    state
}

fn encoded(value: &impl Serializable) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.serialized_size());
    value
        .serialize(&mut bytes)
        .expect("serializing to a Vec does not fail");
    bytes
}

/// The x-coordinate of a member's secp256k1 point. One secret key signs for
/// every member with the same x: the Schnorr key, and both ECDSA parities
/// (`02 || x` with `d`, `03 || x` with `n - d`).
fn member_x(member: &ContractMaintenanceVerifyingKey) -> Vec<u8> {
    match member {
        ContractMaintenanceVerifyingKey::Schnorr(key) => encoded(key),
        // The SEC1 parity byte leads.
        ContractMaintenanceVerifyingKey::ECDSA(key) => encoded(key)[1..].to_vec(),
    }
}

/// Validate a committee + threshold before it is committed on-chain: the
/// committee must be non-empty and `1 <= threshold <= committee.len()`.
///
/// Guards against permanently un-maintainable contracts (`threshold` above the
/// committee size, or an empty committee) and — critically — `threshold == 0`,
/// which the ledger accepts with **zero** signatures (anyone could then govern
/// the contract).
pub(crate) fn validate_committee(
    committee: &[ContractMaintenanceVerifyingKey],
    threshold: u32,
) -> Result<(), ContractError> {
    if committee.is_empty() {
        return Err(ContractError::Maintenance(
            "maintenance committee must have at least one member".into(),
        ));
    }
    if threshold == 0 {
        return Err(ContractError::Maintenance(
            "maintenance threshold must be at least 1 (threshold 0 would let anyone govern the \
             contract)"
                .into(),
        ));
    }
    if threshold as usize > committee.len() {
        return Err(ContractError::Maintenance(format!(
            "maintenance threshold {threshold} exceeds committee size {}; it could never be met",
            committee.len()
        )));
    }
    // Reject duplicate members: a committee like [vk, vk] with threshold 2 would
    // be satisfiable by a single key signing at two indices, collapsing k-of-n.
    // The ledger accepts it. Members are compared by x, so one secret key also
    // cannot sit at two indices as a Schnorr and an ECDSA member.
    let xs: Vec<Vec<u8>> = committee.iter().map(member_x).collect();
    for (i, x) in xs.iter().enumerate() {
        if xs[..i].contains(x) {
            return Err(ContractError::Maintenance(
                "maintenance committee contains a duplicate verifying key".into(),
            ));
        }
    }
    Ok(())
}

/// The low-S form of `signature`, the only one the ledger accepts. Both forms
/// sign the same message under the same key.
fn low_s(signature: ecdsa::Signature) -> ecdsa::Signature {
    let parsed = k256::ecdsa::Signature::from_slice(&encoded(&signature))
        .expect("a ledger ECDSA signature encodes as `r || s`");
    match parsed.normalize_s() {
        Some(low) => ecdsa::Signature::deserialize(&mut &low.to_bytes()[..], 0)
            .expect("k256 encodes a signature as `r || s`"),
        None => signature,
    }
}

/// Check one signature against the committee member it claims, as the ledger
/// will.
fn check_signature(
    data: &[u8],
    index: u32,
    member: &ContractMaintenanceVerifyingKey,
    signature: &ContractMaintenanceSignature,
) -> Result<(), ContractError> {
    let verifies = match (member, signature) {
        (
            ContractMaintenanceVerifyingKey::Schnorr(key),
            ContractMaintenanceSignature::Schnorr(signature),
        ) => key.verify(data, signature),
        (
            ContractMaintenanceVerifyingKey::ECDSA(key),
            ContractMaintenanceSignature::ECDSA(signature),
        ) => key.verify(data, signature),
        (ContractMaintenanceVerifyingKey::Schnorr(_), ContractMaintenanceSignature::ECDSA(_)) => {
            return Err(ContractError::Maintenance(format!(
                "committee index {index} holds a Schnorr key, but its signature is ECDSA"
            )));
        }
        (ContractMaintenanceVerifyingKey::ECDSA(_), ContractMaintenanceSignature::Schnorr(_)) => {
            return Err(ContractError::Maintenance(format!(
                "committee index {index} holds an ECDSA key, but its signature is Schnorr"
            )));
        }
    };
    if verifies {
        Ok(())
    } else {
        Err(ContractError::Maintenance(format!(
            "signature for committee index {index} does not verify"
        )))
    }
}

/// Validate that the signatures over `data` will satisfy `committee` /
/// `threshold` the way the ledger does: each signature's committee index is
/// in range and **distinct**, each verifies over `data`, and the count of
/// distinct valid signatures meets the threshold. Turns on-chain rejections
/// (`NotNormalized` / `KeyNotInCommittee` / `InvalidCommitteeSignature` /
/// `ThresholdMissed`) into early, specific errors.
fn validate_signatures(
    data: &[u8],
    signatures: &[(u32, ContractMaintenanceSignature)],
    committee: &[ContractMaintenanceVerifyingKey],
    threshold: u32,
) -> Result<(), ContractError> {
    let mut seen = std::collections::HashSet::new();
    for (idx, sig) in signatures {
        let member = committee.get(*idx as usize).ok_or_else(|| {
            ContractError::Maintenance(format!(
                "signature index {idx} is outside the committee (size {})",
                committee.len()
            ))
        })?;
        if !seen.insert(*idx) {
            return Err(ContractError::Maintenance(format!(
                "duplicate signature for committee index {idx}"
            )));
        }
        check_signature(data, *idx, member, sig)?;
    }
    if (seen.len() as u32) < threshold {
        return Err(ContractError::Maintenance(format!(
            "not enough valid signatures: have {}, authority threshold is {threshold}",
            seen.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Preconditions, checked against the fetched on-chain state.
// ---------------------------------------------------------------------------

/// Entry-point key for a circuit name, as stored in `ContractState.operations`.
fn entry_point(circuit: &str) -> EntryPointBuf {
    circuit.as_bytes().into()
}

/// Validate a sequence of verifier-key operations against the on-chain state,
/// simulating each in order so a batch is checked the way the ledger applies it.
///
/// `ops` is `(circuit, is_insert)` in submission order. Insert requires the
/// circuit absent (it never replaces); remove requires it present. Because the
/// effects are simulated, `remove("x")` then `insert("x", ..)` in one batch is
/// valid even though the lone insert would not be.
fn validate_vk_sequence(
    state: &ContractState<InMemoryDB>,
    ops: &[(&str, bool)],
) -> Result<(), ContractError> {
    let mut presence: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    for &(circuit, is_insert) in ops {
        let present = match presence.get(circuit) {
            Some(p) => *p,
            None => state.operations.contains_key(&entry_point(circuit)),
        };
        if is_insert && present {
            return Err(ContractError::Maintenance(format!(
                "circuit '{circuit}' already has a verifier key; remove it before inserting"
            )));
        }
        if !is_insert && !present {
            return Err(ContractError::Maintenance(format!(
                "circuit '{circuit}' has no verifier key to remove"
            )));
        }
        // Simulate the effect for later steps in the batch.
        presence.insert(circuit, is_insert);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API: Contract::at(..).maintenance()
// ---------------------------------------------------------------------------

/// Builder for one maintenance transaction. Obtained via
/// [`Contract::maintenance`](crate::Contract::maintenance).
///
/// Chain one or more operations — they are applied **in order, atomically** in a
/// single signed update — then call [`Self::prepare`]. Common batch: rotate a
/// verifier key with `remove_verifier_key(c)` then `insert_verifier_key(c, vk)`.
#[must_use = "call .prepare() to build the update"]
pub struct ContractMaintenance<'a, P> {
    contract: &'a Contract<P>,
    specs: Vec<OpSpec>,
}

impl<'a, P> ContractMaintenance<'a, P> {
    pub(crate) fn new(contract: &'a Contract<P>) -> Self {
        Self {
            contract,
            specs: Vec::new(),
        }
    }

    /// Add an insert-verifier-key step. Errors (at `prepare`) if the circuit is
    /// already defined at that point in the batch. `verifier_key` is the raw
    /// bytes of a compiled `*.verifier` artifact.
    pub fn insert_verifier_key(
        mut self,
        circuit: impl Into<String>,
        verifier_key: impl Into<Vec<u8>>,
    ) -> Self {
        self.specs.push(OpSpec::Insert {
            circuit: circuit.into(),
            verifier_key: verifier_key.into(),
        });
        self
    }

    /// Add a remove-verifier-key step. Errors (at `prepare`) if the circuit is
    /// not defined at that point in the batch.
    pub fn remove_verifier_key(mut self, circuit: impl Into<String>) -> Self {
        self.specs.push(OpSpec::Remove {
            circuit: circuit.into(),
        });
        self
    }

    /// Add a replace-authority step installing `committee`/`threshold`. A
    /// committee can mix Schnorr and ECDSA members.
    pub fn replace_authority(
        mut self,
        committee: Vec<ContractMaintenanceVerifyingKey>,
        threshold: u32,
    ) -> Self {
        self.specs.push(OpSpec::Replace {
            committee,
            threshold,
        });
        self
    }

    /// Fetch the current authority state, validate the batch (simulating each
    /// step in order), and build the unsigned maintenance update. The returned
    /// [`PreparedMaintenance`] exposes the bytes each committee member signs.
    pub async fn prepare(self) -> Result<PreparedMaintenance<'a, P>, ContractError>
    where
        P: AsMidnightProvider,
    {
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(self.prepare_inner()).await
    }

    async fn prepare_inner(self) -> Result<PreparedMaintenance<'a, P>, ContractError>
    where
        P: AsMidnightProvider,
    {
        if self.specs.is_empty() {
            return Err(ContractError::Maintenance(
                "no maintenance operations to perform".into(),
            ));
        }

        let provider = self.contract.provider().as_midnight_provider();
        let address_hex = self.contract.address();
        let address = crate::address::parse_contract_address(address_hex)?;

        // Read the current authority at latest (the signed counter must match
        // what the chain will check at submission — not any pinned block).
        let state = crate::state::fetch_state(provider, address_hex, None).await?;
        let counter = state.maintenance_authority.counter;
        let threshold = state.maintenance_authority.threshold;
        let committee = state.maintenance_authority.committee.clone();

        // Validate the verifier-key steps as a sequence (replace steps don't
        // touch the operations map).
        let vk_ops: Vec<(&str, bool)> = self
            .specs
            .iter()
            .filter_map(|s| match s {
                OpSpec::Insert { circuit, .. } => Some((circuit.as_str(), true)),
                OpSpec::Remove { circuit } => Some((circuit.as_str(), false)),
                OpSpec::Replace { .. } => None,
            })
            .collect();
        validate_vk_sequence(&state, &vk_ops)?;

        // At most one ReplaceAuthority per update (a later one would silently
        // overwrite an earlier one on apply), and each new committee must be
        // satisfiable.
        let mut replaces = 0usize;
        for spec in &self.specs {
            if let OpSpec::Replace {
                committee,
                threshold,
            } = spec
            {
                replaces += 1;
                validate_committee(committee, *threshold)?;
            }
        }
        if replaces > 1 {
            return Err(ContractError::Maintenance(
                "a maintenance update may contain at most one replace_authority".into(),
            ));
        }

        let update =
            crate::ledger::maintenance::prepare_update(&state, address, &self.specs, counter)?;
        Ok(PreparedMaintenance {
            contract: self.contract,
            update,
            signatures: Vec::new(),
            committee,
            required_threshold: threshold,
        })
    }
}

/// One step of a maintenance update, before it takes the ledger's types.
pub(crate) enum OpSpec {
    Insert {
        circuit: String,
        verifier_key: Vec<u8>,
    },
    Remove {
        circuit: String,
    },
    Replace {
        committee: Vec<ContractMaintenanceVerifyingKey>,
        threshold: u32,
    },
}

/// An unsigned (or partially-signed) maintenance update. Collect the committee
/// signatures with [`Self::sign`] / [`Self::add_signature`], then
/// [`Self::send`] it, which returns a [`PendingTx`].
///
/// The caller waits on the [`PendingTx`] (as with a `TransactionRequest`).
/// The wait checks the verdict: [`PendingTx::get_receipt`] fails with
/// [`ProviderError::NotApplied`](midnight_provider::ProviderError::NotApplied)
/// when the update did not apply. (A call and a deploy surface the same
/// failure as [`ContractError::TransactionFailed`].)
#[must_use = "does nothing until sent"]
pub struct PreparedMaintenance<'a, P> {
    contract: &'a Contract<P>,
    update: midnight_helpers::MaintenanceUpdate<midnight_helpers::DefaultDB>,
    /// The signatures collected so far, as `(committee index, signature)`.
    signatures: Vec<(u32, ContractMaintenanceSignature)>,
    /// The on-chain committee at prepare time — used to verify attached
    /// signatures before submission.
    committee: Vec<ContractMaintenanceVerifyingKey>,
    required_threshold: u32,
}

impl<'a, P> PreparedMaintenance<'a, P> {
    /// The exact bytes each committee member signs. Distribute these to the
    /// members; collect their signatures via [`Self::add_signature`].
    ///
    /// Both kinds of member sign SHA-256 of these bytes. A Schnorr member signs
    /// it with BIP-340. An ECDSA member signs it as a prehashed digest (for
    /// example a KMS in digest mode) and returns `r || s`.
    pub fn data_to_sign(&self) -> Vec<u8> {
        self.update.data_to_sign()
    }

    /// Attach a signature produced (anywhere) over [`Self::data_to_sign`], at the
    /// signer's position in the on-chain committee. Its kind must match the
    /// member's key. An ECDSA signature with a high S is attached in its low-S
    /// form, the only one the ledger accepts.
    pub fn add_signature(
        mut self,
        committee_index: u32,
        signature: ContractMaintenanceSignature,
    ) -> Self {
        let signature = match signature {
            ContractMaintenanceSignature::Schnorr(signature) => {
                ContractMaintenanceSignature::Schnorr(signature)
            }
            ContractMaintenanceSignature::ECDSA(signature) => {
                ContractMaintenanceSignature::ECDSA(low_s(signature))
            }
        };
        self.signatures.push((committee_index, signature));
        self
    }

    /// Convenience for the local case: sign [`Self::data_to_sign`] with `key` and
    /// attach it at `committee_index`.
    pub fn sign(self, committee_index: u32, key: &ContractMaintenanceSigningKey) -> Self {
        let signature = key.sign(&mut rand::thread_rng(), &self.data_to_sign());
        self.add_signature(committee_index, signature)
    }

    /// Check the attached signatures against the committee/threshold captured at
    /// prepare time — distinct in-range indices, each verifying, count >=
    /// threshold — so an under-signed or malformed set fails here rather than
    /// after paying to build and submit.
    fn check_signatures(&self) -> Result<(), ContractError> {
        validate_signatures(
            &self.data_to_sign(),
            &self.signatures,
            &self.committee,
            self.required_threshold,
        )
    }

    /// Fill, fund, prove and sign the transaction, then send it and return the
    /// [`PendingTx`] to wait on. Errors if fewer than the authority threshold
    /// of signatures have been attached.
    ///
    /// The send goes through
    /// [`MidnightProvider::send_tx_envelope`](midnight_provider::MidnightProvider::send_tx_envelope),
    /// so the fee Dust goes back to the wallet when the node never gets the
    /// update, or when [`PendingTx::get_receipt`] reads
    /// [`SubmitError::Invalid`](midnight_provider::SubmitError::Invalid).
    pub async fn send(self) -> Result<PendingTx, ContractError>
    where
        P: AsMidnightProvider,
        P::Wallet: MidnightWallet,
    {
        let provider = self.contract.provider().as_midnight_provider();
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(async move {
            let transaction = self.build_inner().await?;
            Ok(provider.send_tx_envelope(transaction).await?)
        })
        .await
    }

    async fn build_inner(self) -> Result<TxEnvelope, ContractError>
    where
        P: AsMidnightProvider,
        P::Wallet: MidnightWallet,
    {
        self.check_signatures()?;
        let provider = self.contract.provider().as_midnight_provider();
        // Boxed; see the frame-size note on `MidnightProvider::fund_prove_sign`.
        Box::pin(crate::ledger::maintenance::maintenance_funded(
            provider,
            self.update,
            &self.signatures,
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schnorr;
    use midnight_typed_state::{StateValue, StorageHashMap};

    fn empty_state() -> ContractState<InMemoryDB> {
        ContractState::new(
            StateValue::Array(vec![].into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    fn state_with_circuit(name: &str) -> ContractState<InMemoryDB> {
        use midnight_onchain_runtime::state::ContractOperation;
        let mut state = empty_state();
        state.operations = state
            .operations
            .insert(entry_point(name), ContractOperation::new(None, None));
        state
    }

    fn schnorr_member() -> (
        ContractMaintenanceVerifyingKey,
        ContractMaintenanceSigningKey,
    ) {
        let key = schnorr::SigningKey::sample(rand::thread_rng());
        (
            ContractMaintenanceVerifyingKey::Schnorr(key.verifying_key()),
            ContractMaintenanceSigningKey::Schnorr(key),
        )
    }

    fn ecdsa_member() -> (
        ContractMaintenanceVerifyingKey,
        ContractMaintenanceSigningKey,
    ) {
        let key = ecdsa::SigningKey::sample(rand::thread_rng());
        (
            ContractMaintenanceVerifyingKey::ECDSA(key.verifying_key()),
            ContractMaintenanceSigningKey::ECDSA(key),
        )
    }

    #[test]
    fn validate_committee_enforces_non_empty_and_threshold_bounds() {
        let (a, _) = schnorr_member();
        let (b, _) = ecdsa_member();
        let committee = vec![a.clone(), b];
        assert!(validate_committee(&committee, 2).is_ok());
        assert!(validate_committee(&committee, 1).is_ok());
        assert!(
            validate_committee(&[], 1).is_err(),
            "empty committee is ungovernable"
        );
        assert!(
            validate_committee(&committee[..1], 0).is_err(),
            "threshold 0 = anyone can govern"
        );
        assert!(
            validate_committee(&committee, 3).is_err(),
            "threshold above committee size can never be met"
        );
        assert!(
            validate_committee(&[a.clone(), a.clone()], 2).is_err(),
            "duplicate committee key collapses k-of-n to 1 signer"
        );
    }

    #[test]
    fn validate_committee_refuses_one_secret_as_a_schnorr_and_an_ecdsa_member() {
        let key = schnorr::SigningKey::sample(rand::thread_rng());
        // The same secret's ECDSA key: the even-y point, `02 || x`.
        let sec1 = [&[0x02][..], &encoded(&key.verifying_key())].concat();
        let same_secret = ecdsa::VerifyingKey::deserialize(&mut &sec1[..], 0).unwrap();
        let committee = [
            ContractMaintenanceVerifyingKey::Schnorr(key.verifying_key()),
            ContractMaintenanceVerifyingKey::ECDSA(same_secret),
        ];

        assert!(validate_committee(&committee, 2).is_err());
    }

    #[test]
    fn validate_signatures_enforces_distinct_inrange_verifying_quorum() {
        let (schnorr, k0) = schnorr_member();
        let (ecdsa, k1) = ecdsa_member();
        let (_, other_ecdsa) = ecdsa_member();
        let committee = vec![schnorr, ecdsa];

        let data = b"the bytes the committee signs";
        let check = |sigs: &[(u32, &ContractMaintenanceSigningKey)], threshold| {
            let signatures: Vec<_> = sigs
                .iter()
                .map(|(i, k)| (*i, k.sign(&mut rand::thread_rng(), data)))
                .collect();
            validate_signatures(data, &signatures, &committee, threshold)
        };

        // 2-of-2, one of each kind, both valid and distinct.
        assert!(check(&[(0, &k0), (1, &k1)], 2).is_ok());
        // Under threshold.
        assert!(check(&[(0, &k0)], 2).is_err());
        // Duplicate committee index (would be NotNormalized on-chain). At
        // threshold 1 the quorum check passes, so only the duplicate check
        // can reject it.
        assert!(check(&[(0, &k0), (0, &k0)], 1).is_err());
        // Index outside the committee (KeyNotInCommittee).
        assert!(check(&[(5, &k0)], 1).is_err());
        // A signature of the other kind (committee[0] is Schnorr, k1 is ECDSA).
        assert!(check(&[(0, &k1)], 1).is_err());
        // Wrong ECDSA key at an index.
        assert!(check(&[(1, &other_ecdsa)], 1).is_err());
    }

    #[test]
    fn low_s_turns_a_high_s_signature_into_the_one_the_ledger_accepts() {
        let key = ecdsa::SigningKey::sample(rand::thread_rng());
        let signature = key.sign(b"data");
        let (r, s) = k256::ecdsa::Signature::from_slice(&encoded(&signature))
            .unwrap()
            .split_scalars();
        let high = k256::ecdsa::Signature::from_scalars(r, -*s).unwrap();
        let high = ecdsa::Signature::deserialize(&mut &high.to_bytes()[..], 0).unwrap();

        assert_eq!(low_s(high), signature);
    }

    #[test]
    fn validate_single_insert_and_remove() {
        // insert: ok when absent, err when present
        assert!(validate_vk_sequence(&empty_state(), &[("increment", true)]).is_ok());
        assert!(
            validate_vk_sequence(&state_with_circuit("increment"), &[("increment", true)]).is_err(),
            "inserting an already-defined circuit should error"
        );
        // remove: ok when present, err when absent
        assert!(
            validate_vk_sequence(&state_with_circuit("increment"), &[("increment", false)]).is_ok()
        );
        assert!(
            validate_vk_sequence(&empty_state(), &[("increment", false)]).is_err(),
            "removing a non-existent circuit should error"
        );
    }

    #[test]
    fn validate_batch_simulates_effects_in_order() {
        let present = state_with_circuit("increment");
        // remove then insert the same circuit: valid as a batch (rotation).
        assert!(
            validate_vk_sequence(&present, &[("increment", false), ("increment", true)]).is_ok(),
            "remove-then-insert of the same circuit should be valid"
        );
        // insert then remove a fresh circuit: valid.
        assert!(validate_vk_sequence(&empty_state(), &[("new", true), ("new", false)]).is_ok());
        // inserting the same circuit twice: the second fails (now present).
        assert!(
            validate_vk_sequence(&empty_state(), &[("x", true), ("x", true)]).is_err(),
            "double insert should error on the second"
        );
        // removing twice: the second fails (now absent).
        assert!(
            validate_vk_sequence(&present, &[("increment", false), ("increment", false)]).is_err()
        );
    }
}
