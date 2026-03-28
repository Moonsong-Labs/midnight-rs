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

use crate::StateError;

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
    /// downloading the entire state blob.
    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, Self::Error>;
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
    fn build_query_path_single() {
        let path = build_query_path(&[0]);
        assert_eq!(path, vec!["4001"]);
    }

    #[test]
    fn build_query_path_multi() {
        let path = build_query_path(&[0, 1]);
        assert_eq!(path, vec!["4001", "0101"]);
    }
}
