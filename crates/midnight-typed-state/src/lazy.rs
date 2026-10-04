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

use crate::StateError;
use crate::accessors::{LIST_HEAD, LIST_LENGTH, LIST_TAIL};

// ---------------------------------------------------------------------------
// Trait + types
// ---------------------------------------------------------------------------

/// A query into a contract's state tree.
///
/// Each element in `path` is a hex-encoded serialized `AlignedValue`.
/// Interpreted as array index, map key, or merkle tree position depending
/// on the `StateValue` variant at each level.
#[derive(Debug, Clone)]
pub struct StateQuery {
    pub path: Vec<String>,
}

/// Result of a single state query.
#[derive(Debug, Clone)]
pub struct StateQueryResult {
    pub query: StateQuery,
    pub value: Option<String>,
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

/// Convert a field index to a hex-encoded serialized `AlignedValue` key.
///
/// This matches the format expected by the `query_contract_state` RPC:
/// each path element is a hex-encoded `Serializable::serialize` output
/// of `AlignedValue::from(index as u8)`.
pub fn index_to_query_key(index: usize) -> String {
    let av: midnight_base_crypto::fab::AlignedValue = u8::try_from(index)
        .expect("field index must fit in u8")
        .into();
    let mut buf = Vec::with_capacity(av.serialized_size());
    av.serialize(&mut buf)
        .expect("AlignedValue serialization to Vec never fails");
    hex::encode(buf)
}

/// Convert any value that can be turned into an `AlignedValue` to a
/// hex-encoded query path element.
///
/// This is used for map key lookups and set membership checks -- the key
/// is serialized to `AlignedValue` bytes and hex-encoded, just like field
/// indices, but from an arbitrary typed value instead of a `usize`.
pub fn value_to_query_key(av: &midnight_base_crypto::fab::AlignedValue) -> String {
    let mut buf = Vec::with_capacity(av.serialized_size());
    av.serialize(&mut buf)
        .expect("AlignedValue serialization to Vec never fails");
    hex::encode(buf)
}

/// Build a `StateQuery` path from a slice of field indices.
///
/// For `FieldIndex::Single(idx)`, pass `&[idx]`.
/// For `FieldIndex::Path(p)`, pass `p` directly.
pub fn build_query_path(indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| index_to_query_key(i)).collect()
}

/// Build the query path to the length cell of a `List` field.
///
/// `field` is the field's index path, as for [`build_query_path`]. The
/// [`ListAccessor`](crate::ListAccessor) docs give the `List` layout.
pub fn list_length_path(field: &[usize]) -> Vec<String> {
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
    pub path: Vec<String>,
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
/// Generated lazy accessors call this, then use `cell_value` + `TryFrom<&ValueSlice>`
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
        assert_eq!(index_to_query_key(0), "4001");
        assert_eq!(index_to_query_key(1), "0101");
        assert_eq!(index_to_query_key(2), "0201");
    }

    #[test]
    fn build_query_path_multi() {
        let path = build_query_path(&[0, 1]);
        assert_eq!(path, vec!["4001", "0101"]);
    }
}
