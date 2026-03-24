//! E2E test: lazy contract state queries with struct deserialization.
//!
//! Tests against the egress_jobs contract: Map<Field, EgressJob> where
//! EgressJob { id: Uint<128>, destination: Bytes<32>, token_ref: Bytes<32>, amount: Uint<128>, status: JobStatus }
//!
//! Run: cargo test --test lazy_query -- --ignored --show-output

use midnight_bindgen::{
    cell_value, hex, tagged_deserialize, AlignedValue, InMemoryDB, InvalidBuiltinDecode,
    StateValue, ValueSlice,
};
use midnight_provider::{MidnightProvider, Provider, StateQuery};

// egress_jobs contract deployed on the forked devnet.
// egress_jobs map at field_path [0], job_count counter at field_path [1].
const CONTRACT: &str = "73182e78eb0646d3c5439938e3ccba8e31908bc4e61eff97e057792b68568220";
const FIELD_MAP: &[u32] = &[0];
const FIELD_COUNTER: &[u32] = &[1];

// Fr key encoding: Fr(1) = "0141", Fr(2) = "0241", Fr(3) = "0341"
const KEY_1: &str = "0141";
const KEY_2: &str = "0241";
const KEY_3: &str = "0341";
const KEY_999: &str = "42e70741"; // Fr(999), not inserted

/// EgressJob struct matching the Compact contract definition.
#[derive(Debug, Clone, PartialEq)]
struct EgressJob {
    id: u128,
    destination: [u8; 32],
    token_ref: [u8; 32],
    amount: u128,
    status: u8, // 0 = pending, 1 = completed
}

impl<'a> TryFrom<&'a ValueSlice> for EgressJob {
    type Error = InvalidBuiltinDecode;
    fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
        if vs.0.len() != 5 {
            return Err(InvalidBuiltinDecode("EgressJob: expected 5 atoms"));
        }
        let id = u128::try_from(&vs.0[0])?;
        let destination: [u8; 32] = vs.0[1].0.as_slice().try_into()
            .map_err(|_| InvalidBuiltinDecode("destination: expected 32 bytes"))?;
        let token_ref: [u8; 32] = vs.0[2].0.as_slice().try_into()
            .map_err(|_| InvalidBuiltinDecode("token_ref: expected 32 bytes"))?;
        let amount = u128::try_from(&vs.0[3])?;
        let status = u8::try_from(&vs.0[4])?;
        Ok(Self { id, destination, token_ref, amount, status })
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

#[tokio::test]
#[ignore]
async fn query_counter() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_COUNTER.to_vec(), key: None },
    ]).await.unwrap();

    assert!(r[0].found);
    let counter = u64::try_from(&*decode_cell(r[0].value.as_deref().unwrap()).value).unwrap();
    eprintln!("job_count = {counter}");
    assert_eq!(counter, 3);
}

#[tokio::test]
#[ignore]
async fn query_single_job_and_deserialize() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_1.into()) },
    ]).await.unwrap();

    assert!(r[0].found);
    let job = decode_job(r[0].value.as_deref().unwrap());
    eprintln!("Job 1: id={}, amount={}, status={}", job.id, job.amount, if job.status == 0 { "pending" } else { "completed" });

    assert_eq!(job.id, 1001);
    assert_eq!(job.destination, [0xaa; 32]);
    assert_eq!(job.token_ref, [0xbb; 32]);
    assert_eq!(job.amount, 500000);
    assert_eq!(job.status, 0); // pending
}

#[tokio::test]
#[ignore]
async fn query_multiple_jobs_and_compare() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_1.into()) },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_2.into()) },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_3.into()) },
    ]).await.unwrap();

    assert_eq!(r.len(), 3);
    assert!(r[0].found);
    assert!(r[1].found);
    assert!(r[2].found);

    let job1 = decode_job(r[0].value.as_deref().unwrap());
    let job2 = decode_job(r[1].value.as_deref().unwrap());
    let job3 = decode_job(r[2].value.as_deref().unwrap());

    eprintln!("Job 1: id={}, dest=0x{}, amount={}, status={}", job1.id, hex::encode(&job1.destination[..4]), job1.amount, if job1.status == 0 { "pending" } else { "completed" });
    eprintln!("Job 2: id={}, dest=0x{}, amount={}, status={}", job2.id, hex::encode(&job2.destination[..4]), job2.amount, if job2.status == 0 { "pending" } else { "completed" });
    eprintln!("Job 3: id={}, dest=0x{}, amount={}, status={}", job3.id, hex::encode(&job3.destination[..4]), job3.amount, if job3.status == 0 { "pending" } else { "completed" });

    // Job 1: id=1001, dest=0xaa..., token=0xbb..., amount=500000, pending
    assert_eq!(job1.id, 1001);
    assert_eq!(job1.destination, [0xaa; 32]);
    assert_eq!(job1.token_ref, [0xbb; 32]);
    assert_eq!(job1.amount, 500000);
    assert_eq!(job1.status, 0);

    // Job 2: id=1002, dest=0xcc..., token=0xdd..., amount=1000000, completed
    assert_eq!(job2.id, 1002);
    assert_eq!(job2.destination, [0xcc; 32]);
    assert_eq!(job2.token_ref, [0xdd; 32]);
    assert_eq!(job2.amount, 1000000);
    assert_eq!(job2.status, 1);

    // Job 3: id=1003, dest=0xee..., token=0xff..., amount=250000, pending
    assert_eq!(job3.id, 1003);
    assert_eq!(job3.destination, [0xee; 32]);
    assert_eq!(job3.token_ref, [0xff; 32]);
    assert_eq!(job3.amount, 250000);
    assert_eq!(job3.status, 0);
}

#[tokio::test]
#[ignore]
async fn query_nonexistent_key() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_999.into()) },
    ]).await.unwrap();

    assert!(!r[0].found);
    eprintln!("key=999 not found (correct)");
}

#[tokio::test]
#[ignore]
async fn query_batch_counter_and_jobs() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_COUNTER.to_vec(), key: None },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_1.into()) },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_2.into()) },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_3.into()) },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some(KEY_999.into()) },
    ]).await.unwrap();

    assert_eq!(r.len(), 5);

    // Counter
    let counter = u64::try_from(&*decode_cell(r[0].value.as_deref().unwrap()).value).unwrap();
    assert_eq!(counter, 3);

    // All 3 jobs found
    let job1 = decode_job(r[1].value.as_deref().unwrap());
    let job2 = decode_job(r[2].value.as_deref().unwrap());
    let job3 = decode_job(r[3].value.as_deref().unwrap());
    assert_eq!(job1.id, 1001);
    assert_eq!(job2.id, 1002);
    assert_eq!(job3.id, 1003);

    // Non-existent key
    assert!(!r[4].found);

    eprintln!("batch: counter={counter}, job1.amount={}, job2.amount={}, job3.amount={}, key999=not_found",
        job1.amount, job2.amount, job3.amount);
}
