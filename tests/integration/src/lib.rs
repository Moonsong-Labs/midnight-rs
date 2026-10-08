compact_bindgen::contract!(
    Gateway,
    "../fixtures/compiled/gateway/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Counter,
    "../../crates/midnight-contract/tests/fixtures/counter/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Election,
    "../../crates/midnight-contract/tests/fixtures/election/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Tiny,
    "../../crates/midnight-contract/tests/fixtures/tiny/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    ManyFields,
    "../fixtures/compiled/many-fields/compiler/analyzed-ir.sexp"
);
// A Zerocash witness returns a `MerkleTreePath`, whose `path` field is a
// `Vector<32, MerkleTreePathEntry>`. The generated struct for it needs the
// `Aligned` / `TryFrom<&ValueSlice>` impls for a vector of a compound type.
compact_bindgen::contract!(
    Zerocash,
    "../fixtures/compiled/zerocash/compiler/analyzed-ir.sexp"
);
// The only fixture with witnesses, so the only one that exercises the witness
// trait and private-state plumbing at compile time.
compact_bindgen::contract!(
    Bboard,
    "../../crates/midnight-contract/tests/fixtures/bboard/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Containers,
    "../conformance/fixtures/containers/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Defaults,
    "../conformance/fixtures/defaults/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Indexing,
    "../conformance/fixtures/indexing/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Structs,
    "../conformance/fixtures/structs/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Trees,
    "../conformance/fixtures/trees/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Kernel,
    "../conformance/fixtures/kernel/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Loops,
    "../conformance/fixtures/loops/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(Ops, "../conformance/fixtures/ops/compiler/analyzed-ir.sexp");
compact_bindgen::contract!(
    Peers,
    "../conformance/fixtures/peers/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Scopes,
    "../conformance/fixtures/scopes/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Shadowing,
    "../conformance/fixtures/shadowing/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Slices,
    "../conformance/fixtures/slices/compiler/analyzed-ir.sexp"
);
compact_bindgen::contract!(
    Vectors,
    "../conformance/fixtures/vectors/compiler/analyzed-ir.sexp"
);

#[cfg(test)]
mod synthetic_tests {
    use compact_bindgen::{
        AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, MerkleTree,
        StateValue, StorageHashMap, TransientFr,
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
    pub(crate) mod election_tests {
        use super::*;
        use crate::election::Election;
        use compact_bindgen::Bytes;

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
        pub(crate) fn election_state(
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
    }

    // ---------------------------------------------------------------
    // Enum cells: every variant decodes by its declaration index
    // ---------------------------------------------------------------
    mod enum_cell_tests {
        use super::*;
        use crate::election::{Election, PublicState};
        use crate::tiny::{STATE, Tiny};

        #[test]
        fn enum_cells_decode_every_variant() {
            for (index, expected) in [
                (0, PublicState::Setup),
                (1, PublicState::Commit),
                (2, PublicState::Reveal),
                (3, PublicState::Final),
            ] {
                let root = election_tests::election_state([0u8; 32], index, 0, 0);
                let ledger = Election::new(make_state(root));
                assert_eq!(
                    ledger.state().expect("state"),
                    expected,
                    "PublicState {index}"
                );
            }

            for (index, expected) in [(0u8, STATE::Unset), (1, STATE::Set)] {
                let root = StateValue::Array(
                    vec![
                        StateValue::from(AlignedValue::from([0u8; 32])),
                        StateValue::from(AlignedValue::from(TransientFr::from(0u64))),
                        StateValue::from(AlignedValue::from(index)),
                    ]
                    .into(),
                );
                let ledger = Tiny::new(make_state(root));
                assert_eq!(ledger.state().expect("state"), expected, "STATE {index}");
            }
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
        use crate::many_fields::{ManyFields, ManyFieldsInitialState};

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

        /// The state a deploy ships has to nest the way the artifact does.
        /// `build()` used to flatten every cell into one array, which the
        /// path accessors above (and the compiled circuits) do not address —
        /// and which, past sixteen cells, the runtime refuses outright.
        #[test]
        fn many_fields_initial_state_is_addressed_by_path() {
            let values: [u64; 16] = core::array::from_fn(|i| (i + 1) as u64 * 10);
            let initial = ManyFieldsInitialState {
                f01: values[0],
                f02: values[1],
                f03: values[2],
                f04: values[3],
                f05: values[4],
                f06: values[5],
                f07: values[6],
                f08: values[7],
                f09: values[8],
                f10: values[9],
                f11: values[10],
                f12: values[11],
                f13: values[12],
                f14: values[13],
                f15: values[14],
                f16: values[15],
            };

            let ledger = ManyFields::new(initial.build());

            // f01 sits alone in the first segment, f02..f16 in the second.
            assert_eq!(ledger.f01().expect("f01"), 10);
            assert_eq!(ledger.f02().expect("f02"), 20);
            assert_eq!(ledger.f08().expect("f08"), 80);
            assert_eq!(ledger.f15().expect("f15"), 150);
            assert_eq!(ledger.f16().expect("f16"), 160);
        }
    }

    // ---------------------------------------------------------------
    // MerkleTreeAccessor tests — synthetic compound state
    // ---------------------------------------------------------------
    mod merkle_tree_accessor_tests {
        use super::*;
        use compact_bindgen::MerkleTreeAccessor;

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
// Roundtrip: From<T> for AlignedValue  <->  TryFrom<&ValueSlice>
// ===================================================================

#[cfg(test)]
mod encode_roundtrip_tests {
    use crate::gateway::UnclaimedDeposit;
    use compact_bindgen::{AlignedValue, Bytes};

    /// Encode a generated struct via the bindgen-emitted
    /// `From<T> for AlignedValue` impl, then decode it back via the
    /// bindgen-emitted `TryFrom<&ValueSlice>` impl and check equality.
    /// This is the proof that encode-side field order matches the
    /// `Aligned::alignment()` concat order the decoder relies on.
    #[test]
    fn unclaimed_deposit_roundtrip() {
        let original = UnclaimedDeposit {
            amount: 1_234_567_890_u128,
            token_ref: Bytes([0xABu8; 32]),
        };

        let av: AlignedValue = original.clone().into();
        let decoded = UnclaimedDeposit::try_from(&*av.value).expect("decode");

        assert_eq!(original.amount, decoded.amount);
        assert_eq!(original.token_ref.0, decoded.token_ref.0);
    }
}

// ===================================================================
// Encodings: the generated types against the canonical runtime's
// ===================================================================

/// Each test builds a value of a generated type, and compares its encoding
/// with a circuit input that the canonical TS runtime recorded.
#[cfg(test)]
mod golden_encodings {
    use compact_bindgen::{AlignedValue, Uint};
    use conformance::state_json::aligned_value_from_json;
    use serde_json::Value as Json;

    use crate::queue_grows::conformance_file;

    fn read_json(path: &str) -> Json {
        let path = conformance_file(path);
        serde_json::from_str(&std::fs::read_to_string(&path).expect("file readable"))
            .expect("file is JSON")
    }

    /// The value that the canonical runtime recorded at the JSON `pointer` of
    /// the golden of a case.
    fn golden_value(fixture: &str, case: &str, pointer: &str) -> AlignedValue {
        let golden = read_json(&format!("expected/{fixture}/{case}.json"));
        let json = golden.pointer(pointer).expect("the golden has the value");
        aligned_value_from_json(json).expect("the golden value decodes")
    }

    /// The tagged fields of the struct argument of the first step of a case.
    fn first_struct_argument(fixture: &str, case: &str) -> Vec<Json> {
        let case_json = read_json(&format!("cases/{fixture}/{case}.json"));
        case_json["steps"][0]["args"][0]["struct"]
            .as_array()
            .expect("the argument is a struct")
            .clone()
    }

    /// The tagged value of the struct field `name`.
    fn field<'a>(fields: &'a [Json], name: &str) -> &'a Json {
        fields
            .iter()
            .find_map(|entry| entry.get(name))
            .unwrap_or_else(|| panic!("the struct has no field {name}"))
    }

    #[test]
    fn odd_width_uint_fields_encode_at_the_compiler_width() {
        let fields = first_struct_argument("structs", "hash-odd-widths");
        let uint = |name| -> u128 {
            field(&fields, name)["uint"]
                .as_str()
                .expect("a decimal string")
                .parse()
                .expect("a u128")
        };
        let odd = crate::structs::Odd {
            small: Uint::try_from(uint("small")).expect("small fits"),
            medium: Uint::try_from(uint("medium")).expect("medium fits"),
            ranged: Uint::try_from(uint("ranged")).expect("ranged fits"),
        };

        assert_eq!(
            AlignedValue::from(odd),
            golden_value("structs", "hash-odd-widths", "/steps/0/input")
        );
    }

    #[test]
    fn a_contract_value_decodes_and_encodes_at_the_compiler_layout() {
        // Step 1 returns the address that step 0 sends.
        let sent = golden_value("peers", "swap-peer", "/steps/0/input");
        let returned = golden_value("peers", "swap-peer", "/steps/1/output");
        let address = crate::peers::SwapPeerReturn::try_from(&*returned.value)
            .expect("the returned address decodes");
        let call = crate::peers::SwapPeerCall { p: address };

        assert_eq!(AlignedValue::from(call.p), sent);
    }
}

// ===================================================================
// List: the states the canonical runtime leaves
// ===================================================================

/// The `containers/queue-grows` conformance case, as the canonical TS runtime
/// ran it: each step pushes one value to the front of `queue`.
#[cfg(test)]
mod queue_grows {
    use std::path::{Path, PathBuf};

    use compact_bindgen::{InMemoryDB, StateValue, StorageHashMap};
    use conformance::runner::{Fixture, ScriptedWitnesses, run_step, state_from_value};
    use midnight_base_crypto::time::Timestamp;

    use crate::containers::Containers;

    /// The value that step 0 pushes.
    pub(crate) const FIRST_PUSH: u64 = 11;
    /// The value that step 1 pushes.
    pub(crate) const SECOND_PUSH: u64 = 22;

    pub(crate) fn conformance_file(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../conformance")
            .join(path)
    }

    /// The contract state at the JSON `pointer` of the case's golden.
    fn golden_state(pointer: &str) -> StateValue<InMemoryDB> {
        let path = conformance_file("expected/containers/queue-grows.json");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("golden readable"))
                .expect("golden is JSON");
        let json = golden.pointer(pointer).expect("golden has the state");
        conformance::state_json::state_value_from_json(json).expect("golden state decodes")
    }

    /// The contract state that the case's constructor leaves: `queue` is empty.
    fn initial_state() -> StateValue<InMemoryDB> {
        golden_state("/constructor/state/data")
    }

    /// The contract state that `step` of the case leaves.
    pub(crate) fn state_after(step: usize) -> StateValue<InMemoryDB> {
        golden_state(&format!("/steps/{step}/state/data"))
    }

    /// The contract state after `push_queue` pushes each of `values`, in order,
    /// on the case's initial state.
    ///
    /// The SDK's interpreter runs the pushes. `make conformance` checks it
    /// against the canonical runtime on this case.
    pub(crate) fn state_with_pushes(values: &[u64]) -> StateValue<InMemoryDB> {
        let ir = std::fs::read_to_string(conformance_file(
            "fixtures/containers/compiler/analyzed-ir.sexp",
        ))
        .expect("fixture readable");
        let fixture = Fixture::load(&ir).expect("fixture loads");
        let witnesses = ScriptedWitnesses::from_json(None).expect("no witnesses");
        let state = values.iter().fold(
            state_from_value(initial_state(), StorageHashMap::new()),
            |state, value| {
                let arg = serde_json::json!({ "uint": value.to_string() });
                let (_, result) = run_step(
                    &fixture,
                    "push_queue",
                    state,
                    &[arg],
                    &witnesses,
                    Timestamp::from_secs(0),
                )
                .expect("push_queue runs");
                result.state
            },
        );
        state.data.get_ref().clone()
    }

    #[test]
    fn list_reads_front_first() {
        let ledger = Containers::new(state_from_value(state_after(1), StorageHashMap::new()));
        let queue = ledger.queue().expect("queue");

        assert_eq!(queue.len(), 2);
        let values: Vec<u64> = queue.iter().map(|v| v.expect("element")).collect();
        assert_eq!(values, [SECOND_PUSH, FIRST_PUSH]);
        assert_eq!(queue.get(1).expect("index 1").expect("element"), FIRST_PUSH);
        assert!(queue.get(2).is_none());
    }

    #[test]
    fn list_reads_the_empty_node_as_empty() {
        let ledger = Containers::new(state_from_value(initial_state(), StorageHashMap::new()));
        let queue = ledger.queue().expect("queue");

        assert!(queue.is_empty());
        assert_eq!(queue.iter().count(), 0);
        assert!(queue.get(0).is_none());
    }
}

// ===================================================================
// InitialState: the generated defaults against the compiler's
// ===================================================================

/// The conformance fixtures with no constructor. For each one, the canonical
/// runtime's `initialState` leaves the compiler's initial value in every
/// field, and the generated `Default` must build that same state.
#[cfg(test)]
mod initial_state_defaults {
    use std::path::Path;

    use compact_bindgen::{ContractState, InMemoryDB};
    use conformance::report::state_report_json;

    /// The golden constructor state of the first case of `fixture`. Every
    /// case of a fixture starts from the same state.
    fn golden_constructor_state(fixture: &str) -> serde_json::Value {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../conformance/expected")
            .join(fixture);
        let mut cases: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        cases.sort();
        let case = cases.first().expect("the fixture has a golden");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(case).expect("golden readable"))
                .expect("golden is JSON");
        golden["constructor"]["state"].clone()
    }

    #[test]
    fn the_default_initial_state_is_the_compilers() {
        let table: Vec<(&str, ContractState<InMemoryDB>)> = vec![
            (
                "containers",
                crate::containers::ContainersInitialState::default().build(),
            ),
            (
                "defaults",
                crate::defaults::DefaultsInitialState::default().build(),
            ),
            (
                "indexing",
                crate::indexing::IndexingInitialState::default().build(),
            ),
            (
                "structs",
                crate::structs::StructsInitialState::default().build(),
            ),
            ("trees", crate::trees::TreesInitialState::default().build()),
            ("kernel", crate::kernel::KernelInitialState.build()),
            ("loops", crate::loops::LoopsInitialState::default().build()),
            ("ops", crate::ops::OpsInitialState::default().build()),
            ("peers", crate::peers::PeersInitialState::default().build()),
            (
                "scopes",
                crate::scopes::ScopesInitialState::default().build(),
            ),
            (
                "shadowing",
                crate::shadowing::ShadowingInitialState::default().build(),
            ),
            (
                "slices",
                crate::slices::SlicesInitialState::default().build(),
            ),
            (
                "vectors",
                crate::vectors::VectorsInitialState::default().build(),
            ),
        ];

        for (fixture, state) in table {
            assert_eq!(
                state_report_json(&state.data.get()),
                golden_constructor_state(fixture),
                "{fixture}: the generated default is not the compiler's initial state"
            );
        }
    }
}

// ===================================================================
// Lazy query tests: a fake node over a real contract state
// ===================================================================

#[cfg(test)]
mod lazy_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use compact_bindgen::{
        AlignedValue, InMemoryDB, StateValue,
        lazy::{StateQuery, StateQueryProvider, StateQueryResult},
        tagged_serialize,
    };
    use midnight_serialize::Deserializable;

    /// A `StateQueryProvider` that answers each path the way the node's
    /// `resolve_state_path` does: it walks a contract state key by key.
    ///
    /// Of the bounds that `midnight_queryContractState` puts on a call, it
    /// applies only [`NODE_MAX_PATH_DEPTH`].
    ///
    /// Call `k` reads `states[k]`, and every later call reads the last state,
    /// so a test can change the state between two calls.
    struct WalkingNode {
        states: Vec<StateValue<InMemoryDB>>,
        calls: AtomicUsize,
    }

    impl WalkingNode {
        fn new(states: Vec<StateValue<InMemoryDB>>) -> Self {
            Self {
                states,
                calls: AtomicUsize::new(0),
            }
        }
    }

    /// The most keys that the node accepts in one path (`MAX_PATH_DEPTH` in
    /// its RPC API).
    const NODE_MAX_PATH_DEPTH: usize = 16;

    /// The node's refusal of a whole call because one path has more than
    /// [`NODE_MAX_PATH_DEPTH`] keys.
    #[derive(Debug)]
    struct PathTooDeep(usize);

    impl std::fmt::Display for PathTooDeep {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "Path too deep: {} steps exceeds the maximum of {NODE_MAX_PATH_DEPTH}",
                self.0
            )
        }
    }

    impl std::error::Error for PathTooDeep {}

    /// The node's answer to one path: a hex tagged `StateValue`, no value for
    /// an absent map key, or a per-query error.
    fn resolve(root: &StateValue<InMemoryDB>, path: &[String]) -> Result<Option<String>, String> {
        let mut current = root.clone();
        for key in path {
            let bytes = hex::decode(key).map_err(|e| format!("key hex: {e}"))?;
            let key = <AlignedValue as Deserializable>::deserialize(&mut bytes.as_slice(), 0)
                .map_err(|e| format!("failed to deserialize key: {e}"))?;
            current = match &current {
                StateValue::Array(arr) => {
                    let i = u8::try_from(&*key.value)
                        .map_err(|e| format!("invalid array index: {e}"))?;
                    arr.get(usize::from(i))
                        .cloned()
                        .ok_or_else(|| format!("array index {i} out of bounds"))?
                }
                StateValue::Map(map) => match map.get(&key) {
                    Some(value) => (*value).clone(),
                    None => return Ok(None),
                },
                _ => return Err("only array, map, and merkle tree can be indexed".into()),
            };
        }
        if matches!(
            current,
            StateValue::Map(_) | StateValue::BoundedMerkleTree(_)
        ) {
            return Err("path resolves to a collection; provide a deeper path".into());
        }
        let mut buf = Vec::new();
        tagged_serialize(&current, &mut buf).expect("serialize");
        Ok(Some(hex::encode(buf)))
    }

    impl StateQueryProvider for WalkingNode {
        type Error = PathTooDeep;

        async fn query_contract_state(
            &self,
            _address: &str,
            queries: Vec<StateQuery>,
            _at_block_hash: Option<compact_bindgen::lazy::H256>,
        ) -> Result<Vec<StateQueryResult>, PathTooDeep> {
            if let Some(query) = queries
                .iter()
                .find(|query| query.path.len() > NODE_MAX_PATH_DEPTH)
            {
                return Err(PathTooDeep(query.path.len()));
            }
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            let state = &self.states[call.min(self.states.len() - 1)];
            Ok(queries
                .into_iter()
                .map(|query| {
                    let (value, error) = match resolve(state, &query.path) {
                        Ok(value) => (value, None),
                        Err(error) => (None, Some(error)),
                    };
                    StateQueryResult {
                        query,
                        value,
                        error,
                    }
                })
                .collect())
        }
    }

    // ---------------------------------------------------------------
    // Election: lazy cell, counter, and set accessors
    // ---------------------------------------------------------------
    mod election_lazy {
        use super::*;
        use crate::election::{ElectionQuery, FIELD_COMMITTED};
        use crate::synthetic_tests::election_tests::election_state;
        use compact_bindgen::{Bytes, StorageHashMap};

        #[tokio::test]
        async fn lazy_authority() {
            let authority = [0xAAu8; 32];
            let node = WalkingNode::new(vec![election_state(authority, 0, 0, 0)]);
            let query = ElectionQuery::new(node, "mock", None);
            let result: Bytes<32> = query.authority().await.unwrap();
            assert_eq!(*result, authority);
        }

        #[tokio::test]
        async fn lazy_tally_yes() {
            let node = WalkingNode::new(vec![election_state([0u8; 32], 0, 100, 42)]);
            let query = ElectionQuery::new(node, "mock", None);
            assert_eq!(query.tally_yes().await.unwrap(), 100u64);
        }

        #[tokio::test]
        async fn lazy_set_contains() {
            let key = [0x11u8; 32];
            let state = election_state([0u8; 32], 0, 0, 0);
            let StateValue::Array(fields) = &state else {
                panic!("an election state is an array");
            };
            let committed = StorageHashMap::new().insert(AlignedValue::from(key), StateValue::Null);
            let fields = fields
                .insert(FIELD_COMMITTED, StateValue::Map(committed))
                .expect("committed is a field");
            let node = WalkingNode::new(vec![StateValue::Array(fields)]);
            let query = ElectionQuery::new(node, "mock", None);
            assert!(query.committed(Bytes(key)).await.unwrap());
        }

        #[tokio::test]
        async fn lazy_set_not_contains() {
            let node = WalkingNode::new(vec![election_state([0u8; 32], 0, 0, 0)]);
            let query = ElectionQuery::new(node, "mock", None);
            let key = [0x99u8; 32];
            assert!(!query.committed(Bytes(key)).await.unwrap());
        }
    }

    // ---------------------------------------------------------------
    // Gateway: lazy map key lookup
    // ---------------------------------------------------------------
    mod gateway_lazy {
        use super::*;
        use crate::gateway::{GatewayInitialState, GatewayQuery};
        use compact_bindgen::TransientFr;

        fn gateway_state(threshold: u8) -> StateValue<InMemoryDB> {
            let state = GatewayInitialState {
                threshold,
                ..Default::default()
            }
            .build();
            state.data.get_ref().clone()
        }

        #[tokio::test]
        async fn lazy_threshold() {
            let node = WalkingNode::new(vec![gateway_state(5)]);
            let query = GatewayQuery::new(node, "mock", None);
            assert_eq!(query.threshold().await.unwrap(), 5u8);
        }

        #[tokio::test]
        async fn lazy_map_key_not_found() {
            let node = WalkingNode::new(vec![gateway_state(5)]);
            let query = GatewayQuery::new(node, "mock", None);
            let result = query.egress_jobs(TransientFr::from(999u64)).await.unwrap();
            assert!(result.is_none());
        }
    }

    // ---------------------------------------------------------------
    // Containers: lazy List element reads
    // ---------------------------------------------------------------
    mod containers_lazy {
        use super::*;
        use crate::containers::ContainersQuery;
        use crate::queue_grows::{FIRST_PUSH, state_after, state_with_pushes};

        #[tokio::test]
        async fn lazy_list_counts_from_the_front() {
            let node = WalkingNode::new(vec![state_after(1)]);
            let query = ContainersQuery::new(node, "mock", None);
            assert_eq!(query.queue(1).await.unwrap(), Some(FIRST_PUSH));
            // Index 2 ends at the empty node's `Null` head, and index 3 runs
            // through its `Null` tail. The length decides both.
            for index in [2, 3] {
                assert_eq!(query.queue(index).await.unwrap(), None, "index {index}");
            }
        }

        #[tokio::test]
        async fn lazy_list_reads_past_the_node_path_bound() {
            // Enough elements that the deepest ones need a longer path than
            // the node accepts.
            let pushes: Vec<u64> = (100..119).collect();
            let node = WalkingNode::new(vec![state_with_pushes(&pushes)]);
            let query = ContainersQuery::new(node, "mock", None);
            for (index, &pushed) in pushes.iter().rev().enumerate() {
                assert_eq!(
                    query.queue(index).await.unwrap(),
                    Some(pushed),
                    "index {index}"
                );
            }
            assert_eq!(query.queue(pushes.len()).await.unwrap(), None);
        }

        #[tokio::test]
        async fn lazy_list_reads_length_and_element_from_one_state() {
            let node = WalkingNode::new(vec![state_after(1), state_after(0)]);
            let query = ContainersQuery::new(node, "mock", None);
            assert_eq!(query.queue(1).await.unwrap(), Some(FIRST_PUSH));
        }
    }
}
