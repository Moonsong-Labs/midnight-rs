//! Each instantiation of `Maybe` keeps its own value type. The election
//! contract has a `Maybe<Opaque<"string">>` ledger cell, and witnesses that
//! return a `Maybe<MerkleTreePath<10, Bytes<32>>>`. The interpreter reads
//! `is_some` out of a witness's `Maybe` by the Compact layout.

compact_bindgen::contract!(
    "../../../../crates/midnight-contract/tests/fixtures/election/compiler/analyzed-ir.sexp"
);

use compact_bindgen::midnight_contract::runtime::Maybe;
use compact_bindgen::{Bytes, TransientFr, Vector};

struct Voter {
    listed: bool,
}

fn any_path() -> MerkleTreePath {
    let entry = MerkleTreePathEntry {
        sibling: MerkleTreeDigest {
            field: TransientFr::from(0u64),
        },
        goes_left: false,
    };
    MerkleTreePath {
        leaf: Bytes([0; 32]),
        path: Vector([(); 10].map(|_| entry.clone())),
    }
}

impl Witnesses for Voter {
    type PrivateState = u64;

    fn private_secret_key(&self, _: &mut u64) -> Result<Bytes<32>, WitnessError> {
        Ok(Bytes([7; 32]))
    }

    fn private_state(&self, _: &mut u64) -> Result<PrivateState, WitnessError> {
        Ok(PrivateState::Initial)
    }

    fn private_state_advance(&self, _: &mut u64) -> Result<(), WitnessError> {
        Ok(())
    }

    fn private_vote_record(&self, _: &mut u64, _: PermissibleVotes) -> Result<(), WitnessError> {
        Ok(())
    }

    fn private_vote(&self, _: &mut u64) -> Result<PermissibleVotes, WitnessError> {
        Ok(PermissibleVotes::Yes)
    }

    fn context_eligible_voters_path_of(
        &self,
        _: &mut u64,
        _: Bytes<32>,
    ) -> Result<Maybe<MerkleTreePath>, WitnessError> {
        Ok(Maybe {
            is_some: self.listed,
            value: any_path(),
        })
    }

    fn context_committed_votes_path_of(
        &self,
        _: &mut u64,
        _: Bytes<32>,
    ) -> Result<Maybe<MerkleTreePath>, WitnessError> {
        Ok(Maybe {
            is_some: false,
            value: any_path(),
        })
    }
}

fn add_voter_error(listed: bool) -> String {
    let voter = Voter { listed };
    let mut simulator = Simulator::new(LedgerInitialState::default(), std::time::UNIX_EPOCH)
        .with_witnesses(&voter);
    simulator.add_voter(Bytes([1; 32])).unwrap_err().to_string()
}

fn main() {
    let listed = add_voter_error(true);
    assert!(listed.contains("Attempted to add a voter twice"), "{listed}");
    let unlisted = add_voter_error(false);
    assert!(unlisted.contains("without authorization"), "{unlisted}");
}
