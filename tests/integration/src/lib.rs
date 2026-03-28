midnight_bindgen::contract!(Gateway, "../../tests/fixtures/gateway-contract-info.json");
midnight_bindgen::contract!(
    Counter,
    "../../tests/fixtures/compiled/counter/compiler/contract-info.json"
);
midnight_bindgen::contract!(
    Election,
    "../../tests/fixtures/compiled/election/compiler/contract-info.json"
);
midnight_bindgen::contract!(
    Tiny,
    "../../tests/fixtures/compiled/tiny/compiler/contract-info.json"
);
midnight_bindgen::contract!(
    ManyFields,
    "../../tests/fixtures/compiled/many-fields/compiler/contract-info.json"
);

#[cfg(test)]
mod tests {
    use super::gateway::*;
    use midnight_bindgen::{ContractState, InMemoryDB, tagged_deserialize};

    fn indexer_url() -> Option<String> {
        std::env::var("MIDNIGHT_INDEXER_URL").ok()
    }

    fn contract_address() -> Option<String> {
        std::env::var("MIDNIGHT_CONTRACT_ADDRESS").ok().or_else(|| {
            std::env::var("MIDNIGHT_CONTRACT_ADDRESS_FILE")
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }

    fn fetch_contract_state(indexer_url: &str, address: &str) -> String {
        let client = reqwest::blocking::Client::new();
        let query = serde_json::json!({
            "query": "query($address: HexEncoded!) { contractAction(address: $address) { state } }",
            "variables": { "address": address }
        });

        let resp: serde_json::Value = client
            .post(format!("{indexer_url}/api/v3/graphql"))
            .json(&query)
            .send()
            .expect("indexer request failed")
            .json()
            .expect("invalid JSON response");

        resp["data"]["contractAction"]["state"]
            .as_str()
            .expect("no state in response")
            .to_string()
    }

    fn deserialize_hex_state(hex_state: &str) -> ContractState<InMemoryDB> {
        let bytes = hex::decode(hex_state).expect("invalid hex");
        tagged_deserialize(&*bytes).expect("deserialization failed")
    }

    macro_rules! require_devnet {
        () => {{
            let url = match indexer_url() {
                Some(u) => u,
                None => {
                    eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                    return;
                }
            };
            let addr = match contract_address() {
                Some(a) => a,
                None => {
                    eprintln!("skipping: MIDNIGHT_CONTRACT_ADDRESS not set");
                    return;
                }
            };
            (url, addr)
        }};
    }

    #[test]
    fn deserialize_gateway_state() {
        let (url, addr) = require_devnet!();
        let hex_state = fetch_contract_state(&url, &addr);

        let state = deserialize_hex_state(&hex_state);
        let ledger = Gateway::new(state);

        let threshold = ledger.threshold().expect("threshold");
        eprintln!("threshold: {threshold}");

        let signing_fee = ledger.signing_fee().expect("signing_fee");
        eprintln!("signing_fee: {signing_fee}");

        let egress_jobs = ledger.egress_jobs().expect("egress_jobs");
        eprintln!("egress_jobs: {} entries", egress_jobs.size());

        let attestations = ledger
            .processed_attestations()
            .expect("processed_attestations");
        eprintln!("processed_attestations: {} entries", attestations.size());
    }
}

#[cfg(test)]
mod synthetic_tests {
    use midnight_bindgen::{
        AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, MerkleTree,
        StateValue, StorageArray, StorageHashMap, TransientFr,
    };

    /// Helper: build a `ContractState` from a root `StateValue`.
    fn make_state(root: StateValue<InMemoryDB>) -> ContractState<InMemoryDB> {
        ContractState::new(
            root,
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }

    // ---------------------------------------------------------------
    // Counter contract: 1 field (round: counter at index 0)
    // ---------------------------------------------------------------
    mod counter_tests {
        use super::*;
        use crate::counter::Counter;

        #[test]
        fn counter_round_zero() {
            let root = StateValue::Array(vec![StateValue::from(0u64)].into());
            let state = make_state(root);
            let ledger = Counter::new(state);
            assert_eq!(ledger.round().expect("round"), 0u64);
        }

        #[test]
        fn counter_round_nonzero() {
            let root = StateValue::Array(vec![StateValue::from(42u64)].into());
            let state = make_state(root);
            let ledger = Counter::new(state);
            assert_eq!(ledger.round().expect("round"), 42u64);
        }

        #[test]
        fn counter_round_max_u64() {
            let root = StateValue::Array(vec![StateValue::from(u64::MAX)].into());
            let state = make_state(root);
            let ledger = Counter::new(state);
            assert_eq!(ledger.round().expect("round"), u64::MAX);
        }
    }

    // ---------------------------------------------------------------
    // Election contract: 9 fields
    //   0: authority  (cell, Bytes<32>)
    //   1: state      (cell, Enum PublicState)
    //   2: topic      (cell, Struct Maybe{is_some: bool, value: Opaque})
    //   3: tally_yes  (counter, Uint<64>)
    //   4: tally_no   (counter, Uint<64>)
    //   5: committed_votes (merkle-tree) -- use Null placeholder
    //   6: eligible_voters (merkle-tree) -- use Null placeholder
    //   7: committed  (set, Bytes<32>)
    //   8: revealed   (set, Bytes<32>)
    // ---------------------------------------------------------------
    mod election_tests {
        use super::*;
        use crate::election::{Election, PublicState};
        use midnight_bindgen::Bytes;

        /// Build a compound `StateValue::Array` for a merkle-tree field.
        ///
        /// Layout: `[BoundedMerkleTree(blank), Cell(first_free=0), Map(empty)]`
        pub(super) fn empty_merkle_tree_value(depth: u8) -> StateValue<InMemoryDB> {
            StateValue::Array(
                vec![
                    StateValue::BoundedMerkleTree(MerkleTree::blank(depth)),
                    StateValue::from(0u64),
                    StateValue::Map(StorageHashMap::new()),
                ]
                .into(),
            )
        }

        /// Build a 9-element election state array.
        pub(super) fn election_state(
            authority: [u8; 32],
            state_variant: u8,
            tally_yes: u64,
            tally_no: u64,
        ) -> StateValue<InMemoryDB> {
            StateValue::Array(
                vec![
                    // 0: authority (cell, Bytes<32>)
                    StateValue::from(AlignedValue::from(authority)),
                    // 1: state (cell, Enum PublicState as u8)
                    StateValue::from(AlignedValue::from(state_variant)),
                    // 2: topic (cell, Struct Maybe) -- skip testing, use a placeholder
                    //    We need a valid AlignedValue for Maybe{is_some: bool, value: Opaque("string")}
                    //    Opaque("string") maps to Vec<u8> which has Compress alignment.
                    //    Build: concat(bool=false, empty_compress)
                    StateValue::from(AlignedValue::from(false)),
                    // 3: tally_yes (counter)
                    StateValue::from(tally_yes),
                    // 4: tally_no (counter)
                    StateValue::from(tally_no),
                    // 5: committed_votes (merkle-tree, depth=10)
                    empty_merkle_tree_value(10),
                    // 6: eligible_voters (merkle-tree, depth=10)
                    empty_merkle_tree_value(10),
                    // 7: committed (set, empty)
                    StateValue::Map(StorageHashMap::new()),
                    // 8: revealed (set, empty)
                    StateValue::Map(StorageHashMap::new()),
                ]
                .into(),
            )
        }

        #[test]
        fn election_authority_bytes() {
            let authority = [0xAAu8; 32];
            let root = election_state(authority, 0, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let result = ledger.authority().expect("authority");
            assert_eq!(*result, authority);
        }

        #[test]
        fn election_state_enum_setup() {
            let root = election_state([0u8; 32], 0, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let result = ledger.state().expect("state");
            assert_eq!(result, PublicState::Setup);
        }

        #[test]
        fn election_state_enum_commit() {
            let root = election_state([0u8; 32], 1, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let result = ledger.state().expect("state");
            assert_eq!(result, PublicState::Commit);
        }

        #[test]
        fn election_state_enum_reveal() {
            let root = election_state([0u8; 32], 2, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let result = ledger.state().expect("state");
            assert_eq!(result, PublicState::Reveal);
        }

        #[test]
        fn election_state_enum_final() {
            let root = election_state([0u8; 32], 3, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let result = ledger.state().expect("state");
            assert_eq!(result, PublicState::Final);
        }

        #[test]
        fn election_tally_counters() {
            let root = election_state([0u8; 32], 0, 100, 42);
            let state = make_state(root);
            let ledger = Election::new(state);

            assert_eq!(ledger.tally_yes().expect("tally_yes"), 100u64);
            assert_eq!(ledger.tally_no().expect("tally_no"), 42u64);
        }

        #[test]
        fn election_empty_sets() {
            let root = election_state([0u8; 32], 0, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let committed = ledger.committed().expect("committed");
            assert!(committed.is_empty());
            assert_eq!(committed.size(), 0);

            let revealed = ledger.revealed().expect("revealed");
            assert!(revealed.is_empty());
            assert_eq!(revealed.size(), 0);
        }

        #[test]
        fn election_set_with_entries() {
            let key1 = [0x11u8; 32];
            let key2 = [0x22u8; 32];

            let committed_map = StorageHashMap::new()
                .insert(AlignedValue::from(key1), StateValue::Null)
                .insert(AlignedValue::from(key2), StateValue::Null);

            let root = StateValue::Array(
                vec![
                    StateValue::from(AlignedValue::from([0u8; 32])),
                    StateValue::from(AlignedValue::from(0u8)),
                    StateValue::from(AlignedValue::from(false)),
                    StateValue::from(0u64),
                    StateValue::from(0u64),
                    empty_merkle_tree_value(10),
                    empty_merkle_tree_value(10),
                    StateValue::Map(committed_map),
                    StateValue::Map(StorageHashMap::new()),
                ]
                .into(),
            );
            let state = make_state(root);
            let ledger = Election::new(state);

            let committed = ledger.committed().expect("committed");
            assert_eq!(committed.size(), 2);
            assert!(!committed.is_empty());

            // Iterate set elements and verify conversion
            let mut elements: Vec<Bytes<32>> =
                committed.iter().map(|r| r.expect("set element")).collect();
            elements.sort_by_key(|b| **b);
            assert_eq!(*elements[0], key1);
            assert_eq!(*elements[1], key2);
        }
    }

    // ---------------------------------------------------------------
    // Tiny contract: 3 fields
    //   0: authority (cell, Bytes<32>)
    //   1: value     (cell, Field) -- exported
    //   2: state     (cell, Enum STATE: unset, set)
    // ---------------------------------------------------------------
    mod tiny_tests {
        use super::*;
        use crate::tiny::Tiny;

        #[test]
        fn tiny_authority() {
            let authority = [0xBBu8; 32];
            let root = StateValue::Array(
                vec![
                    StateValue::from(AlignedValue::from(authority)),
                    StateValue::from(AlignedValue::from(TransientFr::from(0u64))),
                    StateValue::from(AlignedValue::from(0u8)),
                ]
                .into(),
            );
            let state = make_state(root);
            let ledger = Tiny::new(state);

            let result = ledger.authority().expect("authority");
            assert_eq!(*result, authority);
        }

        #[test]
        fn tiny_value_field() {
            let field_val = TransientFr::from(12345u64);
            let root = StateValue::Array(
                vec![
                    StateValue::from(AlignedValue::from([0u8; 32])),
                    StateValue::from(AlignedValue::from(field_val)),
                    StateValue::from(AlignedValue::from(0u8)),
                ]
                .into(),
            );
            let state = make_state(root);
            let ledger = Tiny::new(state);

            let result = ledger.value().expect("value");
            assert_eq!(result, TransientFr::from(12345u64));
        }

        #[test]
        fn tiny_state_enum() {
            use crate::tiny::STATE;

            // unset (variant 0)
            let root = StateValue::Array(
                vec![
                    StateValue::from(AlignedValue::from([0u8; 32])),
                    StateValue::from(AlignedValue::from(TransientFr::from(0u64))),
                    StateValue::from(AlignedValue::from(0u8)),
                ]
                .into(),
            );
            let state = make_state(root);
            let ledger = Tiny::new(state);
            assert_eq!(ledger.state().expect("state"), STATE::Unset);

            // set (variant 1)
            let root = StateValue::Array(
                vec![
                    StateValue::from(AlignedValue::from([0u8; 32])),
                    StateValue::from(AlignedValue::from(TransientFr::from(0u64))),
                    StateValue::from(AlignedValue::from(1u8)),
                ]
                .into(),
            );
            let state = make_state(root);
            let ledger = Tiny::new(state);
            assert_eq!(ledger.state().expect("state"), STATE::Set);
        }
    }

    // ---------------------------------------------------------------
    // ManyFields contract: 16 fields (all Cell<Uint<64>>)
    // Exercises B-tree path indices (>15 fields).
    // Layout: root Array has 2 segments:
    //   segment 0: Array([f01])           — path [0, 0]
    //   segment 1: Array([f02..f16])      — paths [1, 0]..[1, 14]
    // ---------------------------------------------------------------
    mod many_fields_tests {
        use super::*;
        use crate::many_fields::ManyFields;

        fn build_many_fields_state(values: [u64; 16]) -> ContractState<InMemoryDB> {
            // Segment 0: 1 field (f01)
            let seg0 = StateValue::Array(vec![StateValue::from(values[0])].into());
            // Segment 1: 15 fields (f02-f16)
            let seg1_fields: Vec<StateValue<InMemoryDB>> =
                values[1..].iter().map(|&v| StateValue::from(v)).collect();
            let seg1 = StateValue::Array(seg1_fields.into());
            // Root array with 2 segments
            let root = StateValue::Array(vec![seg0, seg1].into());
            make_state(root)
        }

        #[test]
        fn many_fields_first_field() {
            let mut values = [0u64; 16];
            values[0] = 100;
            let state = build_many_fields_state(values);
            let ledger = ManyFields::new(state);
            assert_eq!(ledger.f01().expect("f01"), 100u64);
        }

        #[test]
        fn many_fields_last_field() {
            let mut values = [0u64; 16];
            values[15] = 999;
            let state = build_many_fields_state(values);
            let ledger = ManyFields::new(state);
            assert_eq!(ledger.f16().expect("f16"), 999u64);
        }

        #[test]
        fn many_fields_all_distinct() {
            let values: [u64; 16] = core::array::from_fn(|i| (i + 1) as u64 * 10);
            let state = build_many_fields_state(values);
            let ledger = ManyFields::new(state);
            assert_eq!(ledger.f01().expect("f01"), 10);
            assert_eq!(ledger.f02().expect("f02"), 20);
            assert_eq!(ledger.f08().expect("f08"), 80);
            assert_eq!(ledger.f15().expect("f15"), 150);
            assert_eq!(ledger.f16().expect("f16"), 160);
        }
    }

    // ---------------------------------------------------------------
    // ListAccessor tests — synthetic state with Array of Cells
    // ---------------------------------------------------------------
    mod list_accessor_tests {
        use super::*;
        use midnight_bindgen::ListAccessor;

        /// Build a `ListAccessor<u64>` from a vector of u64 values.
        fn make_list(values: &[u64]) -> StorageArray<StateValue<InMemoryDB>, InMemoryDB> {
            let cells: Vec<StateValue<InMemoryDB>> =
                values.iter().map(|&v| StateValue::from(v)).collect();
            cells.into()
        }

        #[test]
        fn list_empty() {
            let arr = make_list(&[]);
            let list: ListAccessor<'_, u64> = ListAccessor::new(&arr);
            assert!(list.is_empty());
            assert_eq!(list.len(), 0);
            assert!(list.get(0).is_none());
            assert_eq!(list.iter().count(), 0);
        }

        #[test]
        fn list_get_elements() {
            let arr = make_list(&[10, 20, 30]);
            let list: ListAccessor<'_, u64> = ListAccessor::new(&arr);
            assert_eq!(list.len(), 3);
            assert!(!list.is_empty());

            assert_eq!(list.get(0).unwrap().unwrap(), 10u64);
            assert_eq!(list.get(1).unwrap().unwrap(), 20u64);
            assert_eq!(list.get(2).unwrap().unwrap(), 30u64);
            assert!(list.get(3).is_none());
        }

        #[test]
        fn list_iter() {
            let arr = make_list(&[100, 200, 300]);
            let list: ListAccessor<'_, u64> = ListAccessor::new(&arr);
            let values: Vec<u64> = list.iter().map(|r| r.unwrap()).collect();
            assert_eq!(values, vec![100u64, 200, 300]);
        }
    }

    // ---------------------------------------------------------------
    // MerkleTreeAccessor tests — synthetic compound state
    // ---------------------------------------------------------------
    mod merkle_tree_accessor_tests {
        use super::*;
        use midnight_bindgen::MerkleTreeAccessor;

        /// Build a compound merkle tree StateValue with given depth and first_free.
        fn make_merkle_tree_state(depth: u8, first_free: u64) -> StateValue<InMemoryDB> {
            StateValue::Array(
                vec![
                    StateValue::BoundedMerkleTree(MerkleTree::blank(depth)),
                    StateValue::from(first_free),
                    StateValue::Map(StorageHashMap::new()),
                ]
                .into(),
            )
        }

        #[test]
        fn merkle_tree_empty() {
            let sv = make_merkle_tree_state(10, 0);
            let accessor = MerkleTreeAccessor::from_state(&sv).expect("from_state");
            assert_eq!(accessor.height(), 10);
            assert_eq!(accessor.first_free(), 0);
        }

        #[test]
        fn merkle_tree_with_first_free() {
            let sv = make_merkle_tree_state(20, 42);
            let accessor = MerkleTreeAccessor::from_state(&sv).expect("from_state");
            assert_eq!(accessor.height(), 20);
            assert_eq!(accessor.first_free(), 42);
        }

        #[test]
        fn merkle_tree_from_non_array_fails() {
            let sv = StateValue::<InMemoryDB>::Null;
            let err = MerkleTreeAccessor::from_state(&sv).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Array") && msg.contains("Null"),
                "unexpected error: {msg}"
            );
        }

        #[test]
        fn merkle_tree_from_wrong_element_fails() {
            // Array with wrong first element (Cell instead of BoundedMerkleTree)
            let sv = StateValue::Array(
                vec![
                    StateValue::from(0u64),
                    StateValue::from(0u64),
                    StateValue::Map(StorageHashMap::new()),
                ]
                .into(),
            );
            let err = MerkleTreeAccessor::from_state(&sv).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("BoundedMerkleTree") && msg.contains("Cell"),
                "unexpected error: {msg}"
            );
        }
    }

    // ---------------------------------------------------------------
    // Election contract: verify MerkleTreeAccessor integration
    // ---------------------------------------------------------------
    mod election_merkle_tree_tests {
        use super::*;
        use crate::election::Election;

        #[test]
        fn election_merkle_tree_accessor() {
            let root = election_tests::election_state([0u8; 32], 0, 0, 0);
            let state = make_state(root);
            let ledger = Election::new(state);

            let committed_votes = ledger.committed_votes().expect("committed_votes");
            assert_eq!(committed_votes.height(), 10);
            assert_eq!(committed_votes.first_free(), 0);
            // Blank trees still have a computed root in midnight-ledger
            let _root = committed_votes.root();

            let eligible_voters = ledger.eligible_voters().expect("eligible_voters");
            assert_eq!(eligible_voters.height(), 10);
            assert_eq!(eligible_voters.first_free(), 0);
        }
    }
}

// ===================================================================
// Lazy query tests — mock StateQueryProvider
// ===================================================================

#[cfg(test)]
mod lazy_tests {
    use std::collections::HashMap;

    use midnight_bindgen::{
        AlignedValue, InMemoryDB, StateValue,
        lazy::{self, StateQuery, StateQueryProvider, StateQueryResult},
        tagged_serialize,
    };

    /// Serialize a `StateValue<InMemoryDB>` to a hex string (same format
    /// the node RPC returns).
    fn sv_to_hex(sv: &StateValue<InMemoryDB>) -> String {
        let mut buf = Vec::new();
        tagged_serialize(sv, &mut buf).expect("serialize");
        hex::encode(buf)
    }

    // ---------------------------------------------------------------
    // Mock provider
    // ---------------------------------------------------------------

    /// A mock provider that maps `(address, path)` → `StateValue`.
    /// The path is joined with `/` for map key convenience.
    struct MockProvider {
        entries: HashMap<String, StateValue<InMemoryDB>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                entries: HashMap::new(),
            }
        }

        /// Insert a state value at the given query path.
        fn insert(mut self, path: &[&str], sv: StateValue<InMemoryDB>) -> Self {
            self.entries.insert(path.join("/"), sv);
            self
        }

        fn path_key(path: &[String]) -> String {
            path.join("/")
        }
    }

    #[derive(Debug)]
    struct MockError(String);
    impl std::fmt::Display for MockError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "mock error: {}", self.0)
        }
    }
    impl std::error::Error for MockError {}

    impl StateQueryProvider for MockProvider {
        type Error = MockError;

        async fn query_contract_state(
            &self,
            _address: &str,
            queries: Vec<StateQuery>,
        ) -> Result<Vec<StateQueryResult>, MockError> {
            Ok(queries
                .into_iter()
                .map(|q| {
                    let key = Self::path_key(&q.path);
                    let value = self.entries.get(&key).map(sv_to_hex);
                    StateQueryResult {
                        query: q,
                        value,
                        error: None,
                    }
                })
                .collect())
        }
    }

    // ---------------------------------------------------------------
    // Counter: lazy round() accessor
    // ---------------------------------------------------------------
    mod counter_lazy {
        use super::*;
        use crate::counter::CounterQuery;

        #[tokio::test]
        async fn lazy_round_zero() {
            let round_path = lazy::build_query_path(&[0]);
            let provider = MockProvider::new().insert(
                &round_path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::from(0u64),
            );
            let query = CounterQuery::new(provider, "mock");
            assert_eq!(query.round().await.unwrap(), 0u64);
        }

        #[tokio::test]
        async fn lazy_round_nonzero() {
            let round_path = lazy::build_query_path(&[0]);
            let provider = MockProvider::new().insert(
                &round_path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::from(42u64),
            );
            let query = CounterQuery::new(provider, "mock");
            assert_eq!(query.round().await.unwrap(), 42u64);
        }
    }

    // ---------------------------------------------------------------
    // Election: lazy cell, counter, and set accessors
    // ---------------------------------------------------------------
    mod election_lazy {
        use super::*;
        use crate::election::ElectionQuery;
        use midnight_bindgen::Bytes;

        #[tokio::test]
        async fn lazy_authority() {
            let authority = [0xAAu8; 32];
            let path = lazy::build_query_path(&[0]);
            let provider = MockProvider::new().insert(
                &path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::from(AlignedValue::from(authority)),
            );
            let query = ElectionQuery::new(provider, "mock");
            let result: Bytes<32> = query.authority().await.unwrap();
            assert_eq!(*result, authority);
        }

        #[tokio::test]
        async fn lazy_tally_yes() {
            let path = lazy::build_query_path(&[3]);
            let provider = MockProvider::new().insert(
                &path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::from(100u64),
            );
            let query = ElectionQuery::new(provider, "mock");
            assert_eq!(query.tally_yes().await.unwrap(), 100u64);
        }

        #[tokio::test]
        async fn lazy_set_contains() {
            // Set stores Null for present keys
            let key = [0x11u8; 32];
            let field_path = lazy::build_query_path(&[7]); // committed = index 7
            let key_hex = lazy::value_to_query_key(&AlignedValue::from(key));
            let mut full_path = field_path.clone();
            full_path.push(key_hex.clone());

            let provider = MockProvider::new().insert(
                &full_path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::Null,
            );
            let query = ElectionQuery::new(provider, "mock");
            assert!(query.committed(AlignedValue::from(key)).await.unwrap());
        }

        #[tokio::test]
        async fn lazy_set_not_contains() {
            // Missing key: provider returns no value, no error
            let provider = MockProvider::new();
            let query = ElectionQuery::new(provider, "mock");
            let key = [0x99u8; 32];
            assert!(!query.committed(AlignedValue::from(key)).await.unwrap());
        }
    }

    // ---------------------------------------------------------------
    // Gateway: lazy map key lookup
    // ---------------------------------------------------------------
    mod gateway_lazy {
        use super::*;
        use crate::gateway::GatewayQuery;

        #[tokio::test]
        async fn lazy_threshold() {
            let path = lazy::build_query_path(&[0]);
            let provider = MockProvider::new().insert(
                &path.iter().map(String::as_str).collect::<Vec<_>>(),
                StateValue::from(AlignedValue::from(5u8)),
            );
            let query = GatewayQuery::new(provider, "mock");
            assert_eq!(query.threshold().await.unwrap(), 5u8);
        }

        #[tokio::test]
        async fn lazy_map_key_not_found() {
            // Empty provider — key not found returns None
            let provider = MockProvider::new();
            let query = GatewayQuery::new(provider, "mock");
            use midnight_bindgen::TransientFr;
            let result = query
                .egress_jobs(AlignedValue::from(TransientFr::from(999u64)))
                .await
                .unwrap();
            assert!(result.is_none());
        }
    }
}
