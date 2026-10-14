//! Lazy state access helpers for generated `{Name}Query<P>` structs.
//!
//! These helpers convert field indices into RPC query paths, execute
//! `query_contract_state`, and decode the results back into typed values.
//!
//! The [`StateQueryProvider`] trait defines the single method needed by
//! generated lazy accessors. Downstream crates (e.g. `midnight-provider`)
//! can implement it for their concrete provider types.

use midnight_onchain_state::state::StateValue;
use midnight_serialize::Serializable;
use midnight_storage::db::InMemoryDB;
pub use primitive_types::H256;
use serde::{Deserializer, Serializer};

use crate::StateError;
use crate::accessors::{LIST_HEAD, LIST_LENGTH, LIST_TAIL};

// ---------------------------------------------------------------------------
// Trait + types
// ---------------------------------------------------------------------------

/// One step of a [`StateQuery`] path: the raw bytes of a state key.
///
/// Serializes as `0x`-prefixed lowercase hex, the form that the
/// `midnight_queryContractState` RPC takes and echoes back. Deserializes from
/// hex with or without the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageKey(pub Vec<u8>);

impl serde::Serialize for StorageKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("0x{}", hex::encode(&self.0)))
    }
}

impl<'de> serde::Deserialize<'de> for StorageKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        let digits = text.strip_prefix("0x").unwrap_or(&text);
        hex::decode(digits)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// A path into a contract's state tree, as the `midnight_queryContractState`
/// RPC takes it: one storage key per step, from the root down.
///
/// Each key is a serialized `AlignedValue`. The node reads it as an array
/// index, a map key, or a Merkle tree position, from the `StateValue`
/// variant at that level.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateQuery {
    pub path: Vec<StorageKey>,
}

/// The node's answer to one [`StateQuery`]: the hex-encoded value it found,
/// or the reason it found none.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateQueryResult {
    pub query: StateQuery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Minimal trait for querying individual fields in a contract's state tree.
///
/// This is the only method generated lazy accessors need. Implement this
/// for your provider type to enable lazy contract state queries.
#[allow(async_fn_in_trait)]
pub trait StateQueryProvider: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Query specific fields/keys in a contract's state tree without
    /// downloading the entire state blob. `at_block_hash` pins the read to
    /// that block; `None` reads the latest state.
    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
        at_block_hash: Option<H256>,
    ) -> Result<Vec<StateQueryResult>, Self::Error>;
}

impl<T: StateQueryProvider> StateQueryProvider for &T {
    type Error = T::Error;

    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
        at_block_hash: Option<H256>,
    ) -> Result<Vec<StateQueryResult>, Self::Error> {
        (**self)
            .query_contract_state(address, queries, at_block_hash)
            .await
    }
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors that can occur when lazily querying contract state via a provider.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    /// The underlying provider returned an error.
    #[error("provider error: {0}")]
    Provider(Box<dyn std::error::Error + Send + Sync>),

    /// The state navigation / deserialization failed.
    #[error(transparent)]
    State(#[from] StateError),

    /// The RPC returned an error for a specific query path.
    #[error("query error: {0}")]
    QueryFailed(String),

    /// The RPC returned no value and no error for a query path.
    #[error("query returned no value")]
    NoValue,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a field index to a query key: the serialized `AlignedValue`.
///
/// This matches the format expected by the `query_contract_state` RPC:
/// each path element is the `Serializable::serialize` output of
/// `AlignedValue::from(index as u8)`.
pub fn index_to_query_key(index: usize) -> StorageKey {
    let av: midnight_base_crypto::fab::AlignedValue = u8::try_from(index)
        .expect("field index must fit in u8")
        .into();
    value_to_query_key(&av)
}

/// Convert any value that can be turned into an `AlignedValue` to a query
/// path element.
///
/// This is used for map key lookups and set membership checks -- the key
/// is serialized to `AlignedValue` bytes, just like field indices, but from
/// an arbitrary typed value instead of a `usize`.
pub fn value_to_query_key(av: &midnight_base_crypto::fab::AlignedValue) -> StorageKey {
    let mut buf = Vec::with_capacity(av.serialized_size());
    av.serialize(&mut buf)
        .expect("AlignedValue serialization to Vec never fails");
    StorageKey(buf)
}

/// Build a `StateQuery` path from a slice of field indices.
///
/// For `FieldIndex::Single(idx)`, pass `&[idx]`.
/// For `FieldIndex::Path(p)`, pass `p` directly.
pub fn build_query_path(indices: &[usize]) -> Vec<StorageKey> {
    indices.iter().map(|&i| index_to_query_key(i)).collect()
}

/// Build the query path to the length cell of a `List` field.
///
/// `field` is the field's index path, as for [`build_query_path`]. The
/// [`ListAccessor`](crate::ListAccessor) docs give the `List` layout.
pub fn list_length_path(field: &[usize]) -> Vec<StorageKey> {
    let mut path = build_query_path(field);
    path.push(index_to_query_key(LIST_LENGTH));
    path
}

/// The most keys that one path of `query_contract_state` can have.
///
/// The node's `midnight_queryContractState` refuses the whole call when any
/// path has more keys (`MAX_PATH_DEPTH` in its RPC API).
const MAX_QUERY_PATH_DEPTH: usize = 16;

/// The query path toward one element of a `List` field.
#[derive(Debug, Clone)]
pub struct ListElementPath {
    /// The path to send.
    pub path: Vec<StorageKey>,
    /// `None` if `path` ends at the element's cell. `Some(n)` if `path` ends
    /// at a list node, and the element is at index `n` of that node: read it
    /// with [`ListAccessor::get`](crate::ListAccessor::get).
    pub tails_left: Option<usize>,
}

/// Build the query path toward the element at `index` of a `List` field,
/// counted from the front.
///
/// The full path walks `index` tail nodes and then reads the head, so it has
/// `field.len() + index + 1` keys. If that is more than the node's path
/// depth limit, the path stops at the deepest tail node that the node accepts. The node then returns that whole node, so the read downloads
/// all the elements from there to the end of the list.
///
/// A path past the last element ends at or runs through a `Null`. Send it in
/// the same query as [`list_length_path`], so that both paths read one state.
/// Then compare `index` with that length.
pub fn list_element_path(field: &[usize], index: usize) -> ListElementPath {
    let mut path = build_query_path(field);
    let max_tails = MAX_QUERY_PATH_DEPTH.saturating_sub(field.len());
    if index < max_tails {
        path.extend(std::iter::repeat_n(index_to_query_key(LIST_TAIL), index));
        path.push(index_to_query_key(LIST_HEAD));
        return ListElementPath {
            path,
            tails_left: None,
        };
    }
    path.extend(std::iter::repeat_n(
        index_to_query_key(LIST_TAIL),
        max_tails,
    ));
    ListElementPath {
        path,
        tails_left: Some(index - max_tails),
    }
}

/// Decode the hex-encoded state value from a query result into a `StateValue`.
///
/// Generated lazy accessors call this, then use [`cell_as`](crate::cell_as)
/// to convert to the target type while the `StateValue` is still alive.
pub fn decode_state_value(
    result: &StateQueryResult,
) -> Result<StateValue<InMemoryDB>, ContractError> {
    if let Some(err) = &result.error {
        return Err(ContractError::QueryFailed(err.clone()));
    }
    let hex_val = result.value.as_deref().ok_or(ContractError::NoValue)?;
    let bytes = hex::decode(hex_val).map_err(|e| StateError::HexDecode(e.to_string()))?;
    let sv: StateValue<InMemoryDB> =
        midnight_serialize::tagged_deserialize(&mut &bytes[..]).map_err(StateError::Deserialize)?;
    Ok(sv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_to_query_key_known_values() {
        // Empirically verified against the midnight node RPC:
        assert_eq!(index_to_query_key(0), StorageKey(vec![0x40, 0x01]));
        assert_eq!(index_to_query_key(1), StorageKey(vec![0x01, 0x01]));
        assert_eq!(index_to_query_key(2), StorageKey(vec![0x02, 0x01]));
    }

    #[test]
    fn build_query_path_multi() {
        let path = build_query_path(&[0, 1]);
        assert_eq!(
            path,
            vec![StorageKey(vec![0x40, 0x01]), StorageKey(vec![0x01, 0x01])]
        );
    }

    /// A key travels as `0x`-prefixed hex, the form the node's RPC takes. A
    /// derived impl sends an array of numbers, which the node rejects.
    #[test]
    fn storage_key_travels_as_prefixed_hex() {
        let key = StorageKey(vec![0x01, 0xab]);
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"0x01ab\"");
        assert_eq!(
            serde_json::from_str::<StorageKey>("\"0x01ab\"").unwrap(),
            key
        );
    }
}
