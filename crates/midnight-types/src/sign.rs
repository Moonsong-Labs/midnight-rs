//! The signatures that a proven transaction still needs.
//!
//! A funding writes a placeholder signature in each slot that an unshielded
//! input or a Dust registration of the wallet needs, so the fee counts the
//! slot. The owner of the slot signs after the proof. The signing data of an
//! intent leaves its signatures out, and the proof leaves it as it is, so a
//! signature made after the proof verifies.

use std::collections::BTreeSet;

use midnight_helpers::{
    DefaultDB, Deserializable, DustActions, DustRegistration, Intent, ProofMarker,
    PureGeneratorPedersen, Signature, SignatureKind, SignatureVerifyingKey, Sp, Transaction,
    UnshieldedOffer, UserAddress, base_crypto,
};

use crate::convert::{IntoLedger, IntoSdk};
use crate::transfer::SpentUtxoKey;
use crate::{ProvenTx, UnshieldedRecipient, WalletError};

type SealedIntent = Intent<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>;
type SealedOffer = Sp<UnshieldedOffer<Signature, DefaultDB>, DefaultDB>;

/// One signature that a proven transaction needs.
///
/// Every slot of one signer in one intent verifies against the same signing
/// data, so one signature fills them all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureRequest {
    /// The intent segment that the signature covers.
    pub segment: u16,
    /// The address of the key that owns the slots: unshielded inputs and Dust
    /// registrations.
    pub signer: UnshieldedRecipient,
    /// The bytes to sign: the signing data of the intent at `segment`.
    pub data: Vec<u8>,
}

impl ProvenTx {
    /// One request for each segment and signer that still holds a
    /// placeholder.
    ///
    /// The funding writes a placeholder in each signature slot of the
    /// unshielded inputs and the Dust registrations that it adds. A request
    /// can also name an intent that another party built, such as in a
    /// sponsored transaction. A signature authorizes every spend and Dust
    /// registration of its signer in its intent. So a wallet signs only the
    /// requests of the intents that it funded. Sign the `data` of such a
    /// request with the key of its `signer`, and give the signature to
    /// [`Self::add_signature`].
    pub fn signature_requests(&self) -> Vec<SignatureRequest> {
        let mut requests = Vec::new();
        for (segment, intent) in self.tx.intents() {
            let signers: BTreeSet<UserAddress> = placeholder_owners(&intent)
                .into_iter()
                .map(UserAddress::from)
                .collect();
            if signers.is_empty() {
                continue;
            }
            let data = signing_data(&intent, segment);
            requests.extend(signers.into_iter().map(|signer| SignatureRequest {
                segment,
                signer: signer.into_sdk(),
                data: data.clone(),
            }));
        }
        requests.sort_by_key(|request| request.segment);
        requests
    }

    /// Put `signature` in every placeholder of the signer of `request`, in
    /// the intent at its segment.
    ///
    /// `signature` is the 64-byte encoding of a signature over the `data` of
    /// `request`, in the scheme of the signer's key: Schnorr or ECDSA.
    ///
    /// # Errors
    ///
    /// [`WalletError::Transfer`] when the intent at the segment of `request`
    /// holds no placeholder of its signer, when `signature` does not decode,
    /// and when it does not verify against the signer's key over the signing
    /// data of that intent. The transaction then stays as it was.
    pub fn add_signature(
        &mut self,
        request: &SignatureRequest,
        signature: [u8; 64],
    ) -> Result<(), WalletError> {
        let no_placeholder = || {
            WalletError::Transfer(format!(
                "the intent at segment {} holds no placeholder of {:?}",
                request.segment, request.signer
            ))
        };
        let Transaction::Standard(tx) = &mut self.tx else {
            return Err(no_placeholder());
        };
        let intent = tx
            .intents
            .get(&request.segment)
            .map(|intent| (*intent).clone())
            .ok_or_else(no_placeholder)?;
        let signer: UserAddress = request.signer.into_ledger();
        let key = placeholder_owners(&intent)
            .into_iter()
            .find(|key| UserAddress::from(key.clone()) == signer)
            .ok_or_else(no_placeholder)?;
        let signature = decode(&key, signature)?;
        // The intent, not `request.data`, says what the slots verify against.
        let data = signing_data(&intent, request.segment);
        if !<Signature as SignatureKind<DefaultDB>>::signature_verify::<()>(&data, key, &signature)
        {
            return Err(WalletError::Transfer(format!(
                "the signature does not verify against the key of {:?} at segment {}",
                request.signer, request.segment
            )));
        }
        tx.intents = tx
            .intents
            .insert(request.segment, signed(&intent, &signer, &signature));
        Ok(())
    }

    /// The unshielded inputs of the signer of `request` that hold a
    /// placeholder at its segment.
    pub(crate) fn inputs_to_sign(&self, request: &SignatureRequest) -> Vec<SpentUtxoKey> {
        let signer: UserAddress = request.signer.into_ledger();
        let Some((_, intent)) = self
            .tx
            .intents()
            .find(|(segment, _)| *segment == request.segment)
        else {
            return Vec::new();
        };
        let mut inputs = Vec::new();
        for offer in offers(&intent) {
            for (index, input) in offer.inputs.iter_deref().enumerate() {
                if holds_placeholder(offer.signatures.get(index))
                    && UserAddress::from(input.owner.clone()) == signer
                {
                    inputs.push(SpentUtxoKey {
                        intent_hash: input.intent_hash.0,
                        output_index: input.output_no,
                    });
                }
            }
        }
        inputs
    }
}

/// The bytes that every signature slot of `intent` at `segment` verifies
/// against.
fn signing_data(intent: &SealedIntent, segment: u16) -> Vec<u8> {
    intent
        .erase_proofs()
        .erase_signatures()
        .data_to_sign(segment)
}

fn offers(intent: &SealedIntent) -> impl Iterator<Item = &SealedOffer> {
    [
        &intent.guaranteed_unshielded_offer,
        &intent.fallible_unshielded_offer,
    ]
    .into_iter()
    .flatten()
}

fn holds_placeholder(signature: Option<&Signature>) -> bool {
    signature == Some(&Signature::default())
}

/// The key of each slot of `intent` that holds a placeholder.
fn placeholder_owners(intent: &SealedIntent) -> Vec<SignatureVerifyingKey> {
    let mut owners = Vec::new();
    for offer in offers(intent) {
        for (index, input) in offer.inputs.iter_deref().enumerate() {
            if holds_placeholder(offer.signatures.get(index)) {
                owners.push(input.owner.clone());
            }
        }
    }
    if let Some(actions) = &intent.dust_actions {
        for registration in actions.registrations.iter_deref() {
            if holds_placeholder(registration.signature.as_deref()) {
                owners.push(registration.night_key.clone());
            }
        }
    }
    owners
}

/// `intent` with `signature` in each placeholder of `signer`. The other slots
/// keep what they hold.
fn signed(intent: &SealedIntent, signer: &UserAddress, signature: &Signature) -> SealedIntent {
    let owned_by_signer = |key: &SignatureVerifyingKey| UserAddress::from(key.clone()) == *signer;
    let sign_offer = |offer: &Option<SealedOffer>| {
        offer.as_ref().map(|offer| {
            let signatures: Vec<Signature> = offer
                .signatures
                .iter_deref()
                .enumerate()
                .map(|(index, current)| {
                    let owner = offer.inputs.get(index).map(|input| &input.owner);
                    if holds_placeholder(Some(current)) && owner.is_some_and(owned_by_signer) {
                        signature.clone()
                    } else {
                        current.clone()
                    }
                })
                .collect();
            Sp::new(UnshieldedOffer {
                inputs: offer.inputs.clone(),
                outputs: offer.outputs.clone(),
                signatures: signatures.into(),
            })
        })
    };
    let dust_actions = intent.dust_actions.as_ref().map(|actions| {
        let registrations: Vec<DustRegistration<Signature, DefaultDB>> = actions
            .registrations
            .iter_deref()
            .map(|registration| {
                let mut registration = registration.clone();
                if holds_placeholder(registration.signature.as_deref())
                    && owned_by_signer(&registration.night_key)
                {
                    registration.signature = Some(Sp::new(signature.clone()));
                }
                registration
            })
            .collect();
        Sp::new(DustActions {
            spends: actions.spends.clone(),
            registrations: registrations.into(),
            ctime: actions.ctime,
        })
    });
    Intent {
        guaranteed_unshielded_offer: sign_offer(&intent.guaranteed_unshielded_offer),
        fallible_unshielded_offer: sign_offer(&intent.fallible_unshielded_offer),
        dust_actions,
        ..intent.clone()
    }
}

/// The signature that `bytes` encode in the scheme of `key`.
fn decode(key: &SignatureVerifyingKey, bytes: [u8; 64]) -> Result<Signature, WalletError> {
    let reader = &mut &bytes[..];
    match key {
        SignatureVerifyingKey::Schnorr(_) => {
            base_crypto::schnorr::Signature::deserialize(reader, 0).map(Signature::Schnorr)
        }
        SignatureVerifyingKey::ECDSA(_) => {
            base_crypto::ecdsa::Signature::deserialize(reader, 0).map(Signature::ECDSA)
        }
    }
    .map_err(|e| WalletError::Transfer(format!("decode the signature: {e}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use midnight_helpers::{
        HashMapStorage, IntentHash, SeedableRng, Serializable, StdRng, Timestamp, UnprovenIntent,
        UnshieldedWallet, UtxoSpend,
    };

    use super::*;
    use crate::build::{Reservation, SpentInputs};
    use crate::hd::Seed;

    fn signer(byte: u8) -> UnshieldedWallet {
        UnshieldedWallet::default(Seed::from_bytes(&[byte; 32]).unwrap().into_ledger())
    }

    fn spend(owner: &UnshieldedWallet, byte: u8) -> UtxoSpend {
        UtxoSpend {
            value: 10,
            owner: owner.verifying_key(),
            type_: crate::NIGHT.into_ledger(),
            intent_hash: IntentHash(midnight_helpers::HashOutput([byte; 32])),
            output_no: 0,
        }
    }

    fn bytes(signature: &Signature) -> [u8; 64] {
        let mut bytes = Vec::new();
        match signature {
            Signature::Schnorr(signature) => signature.serialize(&mut bytes),
            Signature::ECDSA(signature) => signature.serialize(&mut bytes),
        }
        .unwrap();
        bytes.try_into().unwrap()
    }

    /// A proven transaction whose intent at segment 1 spends two inputs of
    /// `alice` and one of `bob`, and registers `alice` for Dust, each slot
    /// with a placeholder.
    fn awaiting(alice: &UnshieldedWallet, bob: &UnshieldedWallet) -> ProvenTx {
        let mut inputs = vec![spend(alice, 1), spend(alice, 2), spend(bob, 3)];
        inputs.sort();
        let offer = UnshieldedOffer {
            signatures: vec![Signature::default(); inputs.len()].into(),
            inputs: inputs.into(),
            outputs: Vec::new().into(),
        };
        let registration = DustRegistration {
            night_key: alice.verifying_key(),
            dust_address: None,
            allow_fee_payment: 0,
            signature: Some(Sp::new(Signature::default())),
        };
        let mut intent: UnprovenIntent<DefaultDB> =
            Intent::empty(&mut StdRng::seed_from_u64(7), Timestamp::from_secs(1));
        intent.guaranteed_unshielded_offer = Some(Sp::new(offer));
        intent.dust_actions = Some(Sp::new(DustActions {
            spends: Vec::new().into(),
            registrations: vec![registration].into(),
            ctime: Timestamp::from_secs(1),
        }));
        let tx: midnight_helpers::UnprovenTransaction<DefaultDB> =
            Transaction::from_intents("undeployed", HashMapStorage::new().insert(1, intent));
        ProvenTx {
            tx: tx.mock_prove().expect("mock prove"),
            reservation: Reservation::new(SpentInputs::default(), |_| {}),
        }
    }

    fn address(wallet: &UnshieldedWallet) -> UnshieldedRecipient {
        wallet.user_address.into_sdk()
    }

    /// The ledger verifies each input and each registration of one key in one
    /// intent against the same data, so one signature must fill all of them.
    /// A fill of the first slot alone, or of the inputs alone, leaves a
    /// placeholder that the node refuses. A fill that ignored the owner would
    /// put the signature in a slot of another key.
    #[test]
    fn one_signature_fills_every_placeholder_of_its_signer_in_an_intent() {
        let (alice, bob) = (signer(1), signer(2));
        let mut tx = awaiting(&alice, &bob);

        let requests = tx.signature_requests();
        assert_eq!(
            requests
                .iter()
                .map(|r| (r.segment, r.signer))
                .collect::<HashSet<_>>(),
            HashSet::from([(1, address(&alice)), (1, address(&bob))])
        );
        let request = requests
            .iter()
            .find(|r| r.signer == address(&alice))
            .unwrap();
        let signature = alice.sign(&mut StdRng::seed_from_u64(1), &request.data);
        tx.add_signature(request, bytes(&signature))
            .expect("the signature verifies");

        let left = tx.signature_requests();
        assert_eq!(
            left.iter().map(|r| r.signer).collect::<Vec<_>>(),
            [address(&bob)]
        );

        let (_, intent) = tx.tx.intents().find(|(segment, _)| *segment == 1).unwrap();
        let erased = intent.erase_proofs().erase_signatures();
        let data = erased.data_to_sign(1);
        let offer = (*intent.guaranteed_unshielded_offer.clone().unwrap()).clone();
        for (index, input) in offer.inputs.iter_deref().enumerate() {
            let slot = offer.signatures.get(index).unwrap();
            let verifies = <Signature as SignatureKind<DefaultDB>>::signature_verify::<()>(
                &data,
                input.owner.clone(),
                slot,
            );
            assert_eq!(
                verifies,
                input.owner == alice.verifying_key(),
                "input {index}: only the slots of alice verify"
            );
        }
        let actions = intent.dust_actions.clone().unwrap();
        let registration = actions.registrations.get(0).unwrap();
        assert!(
            <Signature as SignatureKind<DefaultDB>>::signature_verify::<()>(
                &data,
                registration.night_key.clone(),
                registration.signature.as_deref().unwrap(),
            ),
            "the registration verifies"
        );
    }

    /// The wallet checks the inputs that a request covers against its
    /// reservation before it signs. A cover that ignored the segment would
    /// show the inputs of the wallet's own intent for a request at an intent
    /// of another party. The check would pass, and the wallet would sign a
    /// spend that it did not fund.
    #[test]
    fn a_request_covers_the_placeholder_inputs_of_its_signer_at_its_segment() {
        let (alice, bob) = (signer(1), signer(2));
        let tx = awaiting(&alice, &bob);
        let request = |wallet: &UnshieldedWallet| {
            tx.signature_requests()
                .into_iter()
                .find(|r| r.signer == address(wallet))
                .unwrap()
        };
        let key = |byte: u8| SpentUtxoKey {
            intent_hash: midnight_helpers::HashOutput([byte; 32]),
            output_index: 0,
        };

        let mut by_alice = tx.inputs_to_sign(&request(&alice));
        by_alice.sort_by_key(|input| input.intent_hash.0);
        assert_eq!(by_alice, [key(1), key(2)]);
        assert_eq!(tx.inputs_to_sign(&request(&bob)), [key(3)]);
        let elsewhere = SignatureRequest {
            segment: 2,
            ..request(&alice)
        };
        assert!(tx.inputs_to_sign(&elsewhere).is_empty());
    }

    /// A signature that an HSM made over other data, or with another key,
    /// must stop at the SDK. Put in, it reaches the node, which refuses the
    /// whole transaction with no word on the cause.
    #[test]
    fn a_signature_that_does_not_verify_is_refused() {
        let (alice, bob) = (signer(1), signer(2));
        let mut tx = awaiting(&alice, &bob);
        let request = tx
            .signature_requests()
            .into_iter()
            .find(|r| r.signer == address(&alice))
            .unwrap();

        let by_bob = bob.sign(&mut StdRng::seed_from_u64(1), &request.data);
        let other_data = alice.sign(&mut StdRng::seed_from_u64(1), b"other data");
        for wrong in [by_bob, other_data] {
            assert!(matches!(
                tx.add_signature(&request, bytes(&wrong)),
                Err(WalletError::Transfer(_))
            ));
        }
        assert_eq!(tx.signature_requests().len(), 2, "no slot changed");
    }
}
