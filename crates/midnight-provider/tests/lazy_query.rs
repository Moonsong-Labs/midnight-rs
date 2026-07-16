//! E2E test: lazy contract state queries with struct deserialization.
//!
//! Tests against the egress_jobs contract: Map<Field, EgressJob> where
//! EgressJob { id, destination, token_ref, amount, status }
//!
//! Each query path is a list of hex-encoded serialized AlignedValue keys.
//! The node interprets each key based on the StateValue variant at that level.
//!
//! Run: cargo test --test lazy_query -- --ignored --show-output

use compact_bindgen::{
    AlignedValue, InMemoryDB, InvalidBuiltinDecode, StateValue, ValueSlice, cell_value, hex,
    tagged_deserialize,
};
use midnight_provider::{MidnightProvider, Provider, StateQuery};
use sp_storage::StorageKey;

// egress_jobs contract deployed on the forked devnet.
// State tree: Array[ Map(egress_jobs), Cell(job_count) ]
const CONTRACT: &str = "48dc1515c712548673df8b99cf5f0677b52c46bfa5131cc4b189abf500911088";

// Serialized AlignedValue keys for path navigation:
// u8(0) = "4001" (array index 0 → egress_jobs map)
// u8(1) = "0101" (array index 1 → job_count counter)
const IDX_0: &str = "4001";
const IDX_1: &str = "0101";

// Fr key encoding for map lookups:
// Fr(1) = "0141", Fr(2) = "0241", Fr(3) = "0341"
const KEY_1: &str = "0141";
const KEY_2: &str = "0241";
const KEY_3: &str = "0341";
const KEY_999: &str = "42e70741";

/// EgressJob struct matching the Compact contract definition.
#[derive(Debug, Clone, PartialEq)]
struct EgressJob {
    id: u128,
    destination: [u8; 32],
    token_ref: [u8; 32],
    amount: u128,
    status: u8,
}

impl<'a> TryFrom<&'a ValueSlice> for EgressJob {
    type Error = InvalidBuiltinDecode;
    fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
        if vs.0.len() != 5 {
            return Err(InvalidBuiltinDecode("EgressJob: expected 5 atoms"));
        }
        let id = u128::try_from(&vs.0[0])?;
        let destination: [u8; 32] = vs.0[1]
            .0
            .as_slice()
            .try_into()
            .map_err(|_| InvalidBuiltinDecode("destination: expected 32 bytes"))?;
        let token_ref: [u8; 32] = vs.0[2]
            .0
            .as_slice()
            .try_into()
            .map_err(|_| InvalidBuiltinDecode("token_ref: expected 32 bytes"))?;
        let amount = u128::try_from(&vs.0[3])?;
        let status = u8::try_from(&vs.0[4])?;
        Ok(Self {
            id,
            destination,
            token_ref,
            amount,
            status,
        })
    }
}

fn decode_cell(hex_value: &str) -> AlignedValue {
    let bytes = hex::decode(hex_value).unwrap();
    let sv: StateValue<InMemoryDB> = tagged_deserialize(&mut &bytes[..]).unwrap();
    cell_value(&sv).unwrap().clone()
}

fn decode_job(hex_value: &str) -> EgressJob {
    let bytes = hex::decode(hex_value).unwrap();
    let sv: StateValue<InMemoryDB> = tagged_deserialize(&mut &bytes[..]).unwrap();
    let av = cell_value(&sv).unwrap();
    EgressJob::try_from(&*av.value).unwrap()
}

fn provider() -> MidnightProvider {
    MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088").unwrap()
}

/// Helper: build a query from hex-encoded path steps.
fn q(steps: &[&str]) -> StateQuery {
    StateQuery {
        path: steps
            .iter()
            .map(|s| StorageKey(hex::decode(s).unwrap()))
            .collect(),
    }
}

#[tokio::test]
#[ignore]
async fn query_counter() {
    let p = provider();
    let r = p
        .query_contract_state(CONTRACT, vec![q(&[IDX_1])])
        .await
        .unwrap();

    assert!(r[0].value.is_some());
    let counter = u64::try_from(&*decode_cell(r[0].value.as_deref().unwrap()).value).unwrap();
    eprintln!("job_count = {counter}");
    assert_eq!(counter, 3);
}

#[tokio::test]
#[ignore]
async fn query_single_job_and_deserialize() {
    let p = provider();
    let r = p
        .query_contract_state(CONTRACT, vec![q(&[IDX_0, KEY_1])])
        .await
        .unwrap();

    assert!(r[0].value.is_some());
    let job = decode_job(r[0].value.as_deref().unwrap());
    eprintln!(
        "Job 1: id={}, amount={}, status={}",
        job.id,
        job.amount,
        if job.status == 0 {
            "pending"
        } else {
            "completed"
        }
    );

    assert_eq!(job.id, 1001);
    assert_eq!(job.destination, [0xaa; 32]);
    assert_eq!(job.token_ref, [0xbb; 32]);
    assert_eq!(job.amount, 500000);
    assert_eq!(job.status, 0);
}

#[tokio::test]
#[ignore]
async fn query_multiple_jobs_and_compare() {
    let p = provider();
    let r = p
        .query_contract_state(
            CONTRACT,
            vec![q(&[IDX_0, KEY_1]), q(&[IDX_0, KEY_2]), q(&[IDX_0, KEY_3])],
        )
        .await
        .unwrap();

    assert_eq!(r.len(), 3);
    let job1 = decode_job(r[0].value.as_deref().unwrap());
    let job2 = decode_job(r[1].value.as_deref().unwrap());
    let job3 = decode_job(r[2].value.as_deref().unwrap());

    eprintln!(
        "Job 1: id={}, dest=0x{}, amount={}, status={}",
        job1.id,
        hex::encode(&job1.destination[..4]),
        job1.amount,
        if job1.status == 0 {
            "pending"
        } else {
            "completed"
        }
    );
    eprintln!(
        "Job 2: id={}, dest=0x{}, amount={}, status={}",
        job2.id,
        hex::encode(&job2.destination[..4]),
        job2.amount,
        if job2.status == 0 {
            "pending"
        } else {
            "completed"
        }
    );
    eprintln!(
        "Job 3: id={}, dest=0x{}, amount={}, status={}",
        job3.id,
        hex::encode(&job3.destination[..4]),
        job3.amount,
        if job3.status == 0 {
            "pending"
        } else {
            "completed"
        }
    );

    assert_eq!(
        job1,
        EgressJob {
            id: 1001,
            destination: [0xaa; 32],
            token_ref: [0xbb; 32],
            amount: 500000,
            status: 0
        }
    );
    assert_eq!(
        job2,
        EgressJob {
            id: 1002,
            destination: [0xcc; 32],
            token_ref: [0xdd; 32],
            amount: 1000000,
            status: 1
        }
    );
    assert_eq!(
        job3,
        EgressJob {
            id: 1003,
            destination: [0xee; 32],
            token_ref: [0xff; 32],
            amount: 250000,
            status: 0
        }
    );
}

#[tokio::test]
#[ignore]
async fn query_nonexistent_key() {
    let p = provider();
    let r = p
        .query_contract_state(CONTRACT, vec![q(&[IDX_0, KEY_999])])
        .await
        .unwrap();

    assert!(r[0].value.is_none());
    assert!(r[0].error.is_none());
    eprintln!("key=999 not found (correct)");
}

#[tokio::test]
#[ignore]
async fn query_batch_counter_and_jobs() {
    let p = provider();
    let r = p
        .query_contract_state(
            CONTRACT,
            vec![
                q(&[IDX_1]),          // counter
                q(&[IDX_0, KEY_1]),   // job 1
                q(&[IDX_0, KEY_2]),   // job 2
                q(&[IDX_0, KEY_3]),   // job 3
                q(&[IDX_0, KEY_999]), // not found
            ],
        )
        .await
        .unwrap();

    assert_eq!(r.len(), 5);

    let counter = u64::try_from(&*decode_cell(r[0].value.as_deref().unwrap()).value).unwrap();
    assert_eq!(counter, 3);

    let job1 = decode_job(r[1].value.as_deref().unwrap());
    let job2 = decode_job(r[2].value.as_deref().unwrap());
    let job3 = decode_job(r[3].value.as_deref().unwrap());
    assert_eq!(job1.id, 1001);
    assert_eq!(job2.id, 1002);
    assert_eq!(job3.id, 1003);

    assert!(r[4].value.is_none());

    eprintln!(
        "batch: counter={counter}, job1.amount={}, job2.amount={}, job3.amount={}, key999=not_found",
        job1.amount, job2.amount, job3.amount
    );
}
