//! E2E test: lazy contract state queries with struct deserialization.
//!
//! Tests against the egress_jobs contract: Map<Field, EgressJob> where
//! EgressJob { id, destination, token_ref, amount, status }
//!
//! Each query path is a list of serialized AlignedValue keys.
//! The node interprets each key based on the StateValue variant at that level.
//!
//! Run: cargo test --test lazy_query -- --ignored --show-output

use compact_bindgen::{
    AlignedValue, InvalidBuiltinDecode, TransientFr, ValueSlice, cell_value, hex, lazy,
};
use midnight_provider::{MidnightProvider, Provider, StateQuery, StateQueryResult};

// egress_jobs contract deployed on the forked devnet.
// State tree: Array[ Map(egress_jobs), Cell(job_count) ]
const CONTRACT: &str = "48dc1515c712548673df8b99cf5f0677b52c46bfa5131cc4b189abf500911088";
const EGRESS_JOBS: usize = 0;
const JOB_COUNT: usize = 1;

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

impl EgressJob {
    fn status_label(&self) -> &'static str {
        if self.status == 0 {
            "pending"
        } else {
            "completed"
        }
    }
}

fn decode_cell(result: &StateQueryResult) -> AlignedValue {
    cell_value(&lazy::decode_state_value(result).unwrap())
        .unwrap()
        .clone()
}

fn decode_job(result: &StateQueryResult) -> EgressJob {
    EgressJob::try_from(&*decode_cell(result).value).unwrap()
}

fn provider() -> MidnightProvider {
    MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088")
}

fn job_count_query() -> StateQuery {
    StateQuery {
        path: vec![lazy::index_to_query_key(JOB_COUNT)],
    }
}

fn job_query(id: u64) -> StateQuery {
    StateQuery {
        path: vec![
            lazy::index_to_query_key(EGRESS_JOBS),
            lazy::value_to_query_key(&AlignedValue::from(TransientFr::from(id))),
        ],
    }
}

#[tokio::test]
#[ignore]
async fn query_counter() {
    let p = provider();
    let r = p
        .query_contract_state(CONTRACT, vec![job_count_query()])
        .await
        .unwrap();

    assert!(r[0].value.is_some());
    let counter = u64::try_from(&*decode_cell(&r[0]).value).unwrap();
    eprintln!("job_count = {counter}");
    assert_eq!(counter, 3);
}

#[tokio::test]
#[ignore]
async fn query_single_job_and_deserialize() {
    let p = provider();
    let r = p
        .query_contract_state(CONTRACT, vec![job_query(1)])
        .await
        .unwrap();

    assert!(r[0].value.is_some());
    let job = decode_job(&r[0]);
    eprintln!(
        "Job 1: id={}, amount={}, status={}",
        job.id,
        job.amount,
        job.status_label()
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
        .query_contract_state(CONTRACT, vec![job_query(1), job_query(2), job_query(3)])
        .await
        .unwrap();

    assert_eq!(r.len(), 3);
    let job1 = decode_job(&r[0]);
    let job2 = decode_job(&r[1]);
    let job3 = decode_job(&r[2]);

    eprintln!(
        "Job 1: id={}, dest=0x{}, amount={}, status={}",
        job1.id,
        hex::encode(&job1.destination[..4]),
        job1.amount,
        job1.status_label()
    );
    eprintln!(
        "Job 2: id={}, dest=0x{}, amount={}, status={}",
        job2.id,
        hex::encode(&job2.destination[..4]),
        job2.amount,
        job2.status_label()
    );
    eprintln!(
        "Job 3: id={}, dest=0x{}, amount={}, status={}",
        job3.id,
        hex::encode(&job3.destination[..4]),
        job3.amount,
        job3.status_label()
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
        .query_contract_state(CONTRACT, vec![job_query(999)])
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
                job_count_query(),
                job_query(1),
                job_query(2),
                job_query(3),
                job_query(999),
            ],
        )
        .await
        .unwrap();

    assert_eq!(r.len(), 5);

    let counter = u64::try_from(&*decode_cell(&r[0]).value).unwrap();
    assert_eq!(counter, 3);

    let job1 = decode_job(&r[1]);
    let job2 = decode_job(&r[2]);
    let job3 = decode_job(&r[3]);
    assert_eq!(job1.id, 1001);
    assert_eq!(job2.id, 1002);
    assert_eq!(job3.id, 1003);

    assert!(r[4].value.is_none());

    eprintln!(
        "batch: counter={counter}, job1.amount={}, job2.amount={}, job3.amount={}, key999=not_found",
        job1.amount, job2.amount, job3.amount
    );
}
