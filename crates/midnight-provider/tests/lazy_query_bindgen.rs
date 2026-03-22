//! E2E test: decode queryContractState responses using bindgen-generated types.
//!
//! Run: cargo test --test lazy_query_bindgen -- --ignored --show-output

midnight_bindgen::contract!(JobMap, "tests/job-map-contract-info.json");

use job_map::{JobMap as Ledger, Job};
use midnight_provider::{MidnightProvider, Provider, StateQuery};

const CONTRACT: &str = "a48d350bf0c10a06ec2acaf6ea4b3384961305d888f42264598148e79e4055c4";

fn provider() -> MidnightProvider {
    MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088").unwrap()
}

#[tokio::test]
#[ignore]
async fn decode_job_via_bindgen() {
    let p = provider();

    // Query job id=10 (Fr(10) = "0a41") at field_path [1, 14]
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: vec![1, 14], key: Some("0a41".into()) }
    ]).await.unwrap();

    assert!(r[0].found, "job id=10 should exist");

    // Decode using bindgen's tagged_deserialize + generated Job struct
    let hex = r[0].value.as_deref().unwrap();
    let bytes = midnight_bindgen::hex::decode(hex).unwrap();
    let sv: midnight_bindgen::StateValue<midnight_bindgen::InMemoryDB> =
        midnight_bindgen::tagged_deserialize(&mut &bytes[..]).unwrap();
    let av = midnight_bindgen::cell_value(&sv).unwrap();
    let job = Job::try_from(&*av.value).unwrap();

    eprintln!("Job id=10 decoded via bindgen: a={}, b={}", job.a, job.b);
    assert_eq!(job.a, 100);
    assert_eq!(job.b, 200);
}

#[tokio::test]
#[ignore]
async fn decode_all_fields_via_bindgen() {
    let p = provider();

    // Batch query: counter + all 3 jobs + missing key
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: vec![0, 0], key: None },           // counter f0
        StateQuery { field_path: vec![1, 14], key: Some("0a41".into()) },   // Fr(10)
        StateQuery { field_path: vec![1, 14], key: Some("1441".into()) },   // Fr(20)
        StateQuery { field_path: vec![1, 14], key: Some("411e41".into()) }, // Fr(30)
        StateQuery { field_path: vec![1, 14], key: Some("41e70741".into()) }, // Fr(999)
    ]).await.unwrap();

    // Counter — decode as u64 via bindgen's cell_value
    let counter_hex = r[0].value.as_deref().unwrap();
    let bytes = midnight_bindgen::hex::decode(counter_hex).unwrap();
    let sv: midnight_bindgen::StateValue<midnight_bindgen::InMemoryDB> =
        midnight_bindgen::tagged_deserialize(&mut &bytes[..]).unwrap();
    let av = midnight_bindgen::cell_value(&sv).unwrap();
    let counter = u64::try_from(&*av.value).unwrap();
    eprintln!("counter = {counter}");
    assert_eq!(counter, 3);

    // Decode each job
    let decode_job = |idx: usize| -> Job {
        let hex = r[idx].value.as_deref().unwrap();
        let bytes = midnight_bindgen::hex::decode(hex).unwrap();
        let sv: midnight_bindgen::StateValue<midnight_bindgen::InMemoryDB> =
            midnight_bindgen::tagged_deserialize(&mut &bytes[..]).unwrap();
        let av = midnight_bindgen::cell_value(&sv).unwrap();
        Job::try_from(&*av.value).unwrap()
    };

    let j10 = decode_job(1);
    let j20 = decode_job(2);
    let j30 = decode_job(3);

    eprintln!("Job(10): a={}, b={}", j10.a, j10.b);
    eprintln!("Job(20): a={}, b={}", j20.a, j20.b);
    eprintln!("Job(30): a={}, b={}", j30.a, j30.b);

    assert_eq!((j10.a, j10.b), (100, 200));
    assert_eq!((j20.a, j20.b), (1000, 2000));
    assert_eq!((j30.a, j30.b), (42, 99));

    // Missing key
    assert!(!r[4].found);
    eprintln!("Fr(999): not found (correct)");

    eprintln!("\nFull pipeline: RPC → hex → tagged_deserialize → cell_value → Job {{ a, b }}");
}
