//! A contract's state as a chain of either ledger generation serves it.
//!
//! The bindings read ledger 9's [`ContractState`]. A ledger 8 chain serves the
//! older encoding, which [`decode_contract_state`] converts. The conversion
//! loses nothing: every ledger 8 part has a ledger 9 slot of the same shape.

use std::io::Cursor;

use midnight_coin_structure::coin::{ShieldedTokenType, TokenType, UnshieldedTokenType};
use midnight_coin_structure_ledger_8::coin::TokenType as TokenTypeV8;
use midnight_onchain_state::state::{
    ChargedState, ContractMaintenanceAuthority, ContractMaintenanceVerifyingKey, ContractOperation,
    ContractState, EntryPointBuf,
};
use midnight_onchain_state_ledger_8::state::ContractState as ContractStateV8;
use midnight_serialize::{Tagged, peek_tag, tagged_deserialize, tagged_serialize};
use midnight_storage::db::InMemoryDB;
use midnight_storage::storage::HashMap as StorageHashMap;

use crate::StateError;

/// Decode a contract's state from the tagged bytes a chain of either ledger
/// generation serves.
///
/// # Errors
///
/// [`StateError::Deserialize`] when the bytes carry neither generation's
/// contract-state tag, or do not decode under the tag they carry.
pub fn decode_contract_state(bytes: &[u8]) -> Result<ContractState<InMemoryDB>, StateError> {
    let tag = peek_tag(&mut Cursor::new(bytes))?;
    if tag == ContractState::<InMemoryDB>::tag() {
        return Ok(tagged_deserialize(bytes)?);
    }
    if tag == ContractStateV8::<InMemoryDB>::tag() {
        return from_ledger_8(&tagged_deserialize(bytes)?);
    }
    Err(StateError::Deserialize(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("`{tag}` is not a contract state of ledger 8 or ledger 9"),
    )))
}

fn from_ledger_8(
    old: &ContractStateV8<InMemoryDB>,
) -> Result<ContractState<InMemoryDB>, StateError> {
    // The charged state's encoding is the same in both generations.
    let data: ChargedState<InMemoryDB> = reencode(&old.data)?;

    let mut operations = StorageHashMap::new();
    for entry in old.operations.iter() {
        let (entry_point, op) = &*entry;
        // Ledger 9 keeps a ledger 8 verifier key in `v2`, as the same type.
        let mut converted = ContractOperation::new(None, None);
        converted.v2 = op.v2.clone();
        operations = operations.insert(EntryPointBuf(entry_point.0.clone()), converted);
    }

    let authority = &old.maintenance_authority;
    let maintenance_authority = ContractMaintenanceAuthority {
        committee: authority
            .committee
            .iter()
            .cloned()
            .map(ContractMaintenanceVerifyingKey::Schnorr)
            .collect(),
        threshold: authority.threshold,
        counter: authority.counter,
    };

    let mut balance = StorageHashMap::new();
    for entry in old.balance.iter() {
        let (token, value) = &*entry;
        balance = balance.insert(token_type(token), **value);
    }

    Ok(ContractState {
        data,
        operations,
        maintenance_authority,
        balance,
    })
}

fn token_type(old: &TokenTypeV8) -> TokenType {
    match old {
        TokenTypeV8::Shielded(t) => TokenType::Shielded(ShieldedTokenType(t.0)),
        TokenTypeV8::Unshielded(t) => TokenType::Unshielded(UnshieldedTokenType(t.0)),
        TokenTypeV8::Dust => TokenType::Dust,
    }
}

fn reencode<A, B>(value: &A) -> Result<B, StateError>
where
    A: midnight_serialize::Serializable + Tagged,
    B: midnight_serialize::Deserializable + Tagged,
{
    let mut bytes = Vec::new();
    tagged_serialize(value, &mut bytes)?;
    Ok(tagged_deserialize(&mut bytes.as_slice())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_base_crypto::schnorr::SigningKey;
    use midnight_onchain_state::state::StateValue;
    use midnight_onchain_state_ledger_8::state::{
        ContractMaintenanceAuthority as AuthorityV8, ContractOperation as OperationV8,
        EntryPointBuf as EntryPointV8, StateValue as StateValueV8,
    };

    /// Every part a binding or a later call reads must survive the decode.
    /// Losing a verifier key here surfaces far away, as a call the chain
    /// cannot verify.
    #[test]
    fn ledger_8_state_keeps_its_data_keys_and_committee() {
        let vk_bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../devnet/contracts/counter/compiled/keys/increment.verifier"
        ))
        .unwrap();
        let vk = tagged_deserialize(&mut vk_bytes.as_slice()).unwrap();
        let member = SigningKey::from_bytes(&[7; 32]).unwrap().verifying_key();
        let old = ContractStateV8::<InMemoryDB>::new(
            StateValueV8::Array(vec![StateValueV8::from(5u64)].into()),
            StorageHashMap::new().insert(
                EntryPointV8(b"increment".to_vec()),
                OperationV8::new(Some(vk)),
            ),
            AuthorityV8 {
                committee: vec![member.clone()],
                threshold: 1,
                counter: 3,
            },
        );
        let mut bytes = Vec::new();
        tagged_serialize(&old, &mut bytes).unwrap();

        let new = decode_contract_state(&bytes).unwrap();

        let data: StateValue<InMemoryDB> = reencode(&*old.data.get()).unwrap();
        assert_eq!(*new.data.get(), data);
        let old_op = old
            .operations
            .get(&EntryPointV8(b"increment".to_vec()))
            .unwrap();
        let new_op = new
            .operations
            .get(&EntryPointBuf(b"increment".to_vec()))
            .expect("the operation");
        assert!(new_op.v2.is_some());
        assert_eq!(new_op.v2, old_op.v2);
        assert_eq!(
            new.maintenance_authority.committee,
            vec![ContractMaintenanceVerifyingKey::Schnorr(member)]
        );
        assert_eq!(new.maintenance_authority.counter, 3);
    }
}
