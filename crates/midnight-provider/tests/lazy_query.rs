//! E2E test: lazy contract state queries.
//!
//! Run: cargo test --test lazy_query -- --ignored --show-output

use midnight_bindgen::{
    AlignedValue, InMemoryDB, StateValue,
    cell_value, tagged_deserialize,
};
use midnight_provider::{MidnightProvider, Provider, StateQuery};

// job_map contract: 17 fields, Map<Field, [Uint<64>, Uint<64>]> at [1,14], counter at [0,0]
const CONTRACT: &str = "a48d350bf0c10a06ec2acaf6ea4b3384961305d888f42264598148e79e4055c4";
const FIELD_COUNTER: &[u8] = &[0, 0];
const FIELD_MAP: &[u8] = &[1, 14];

fn decode_cell(hex: &str) -> AlignedValue {
    let bytes = midnight_bindgen::hex::decode(hex).unwrap();
    let sv: StateValue<InMemoryDB> = tagged_deserialize(&mut &bytes[..]).unwrap();
    cell_value(&sv).unwrap().clone()
}

fn decode_u64(hex: &str) -> u64 {
    u64::try_from(&*decode_cell(hex).value).unwrap()
}

fn decode_u64_pair(hex: &str) -> (u64, u64) {
    <(u64, u64)>::try_from(&*decode_cell(hex).value).unwrap()
}

fn provider() -> MidnightProvider {
    MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088").unwrap()
}

#[tokio::test]
#[ignore]
async fn query_counter() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_COUNTER.to_vec(), key: None }
    ]).await.unwrap();
    let counter = decode_u64(r[0].value.as_deref().unwrap());
    eprintln!("counter = {counter}");
    assert_eq!(counter, 3);
}

#[tokio::test]
#[ignore]
async fn query_map_entry_and_decode() {
    let p = provider();
    // Fr(10) = "0a41" — job {a: 100, b: 200}
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("0a41".into()) }
    ]).await.unwrap();
    assert!(r[0].found);

    let (a, b) = decode_u64_pair(r[0].value.as_deref().unwrap());
    eprintln!("job id=10: a={a}, b={b}");
    assert_eq!(a, 100);
    assert_eq!(b, 200);
}

#[tokio::test]
#[ignore]
async fn query_map_not_found() {
    let p = provider();
    // Fr(999) = "41e70741" — not inserted
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("41e70741".into()) }
    ]).await.unwrap();
    assert!(!r[0].found);
    eprintln!("key=999 not found");
}

#[tokio::test]
#[ignore]
async fn query_map_size() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_MAP.to_vec(), key: None }
    ]).await.unwrap();
    assert!(r[0].found);
    let size_hex = r[0].value.as_deref().unwrap();
    let size_bytes = midnight_bindgen::hex::decode(size_hex).unwrap();
    let size = u64::from_le_bytes(size_bytes.try_into().unwrap());
    eprintln!("map size = {size}");
    assert_eq!(size, 3);
}

#[tokio::test]
#[ignore]
async fn query_all_jobs_decoded() {
    let p = provider();
    let r = p.query_contract_state(CONTRACT, vec![
        StateQuery { field_path: FIELD_COUNTER.to_vec(), key: None },
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("0a41".into()) },   // Fr(10): a=100, b=200
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("1441".into()) },   // Fr(20): a=1000, b=2000
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("411e41".into()) }, // Fr(30): a=42, b=99
        StateQuery { field_path: FIELD_MAP.to_vec(), key: Some("41e70741".into()) }, // Fr(999): not found
    ]).await.unwrap();

    assert_eq!(r.len(), 5);

    // Counter
    assert_eq!(decode_u64(r[0].value.as_deref().unwrap()), 3);

    // Job id=10: a=100, b=200
    let (a, b) = decode_u64_pair(r[1].value.as_deref().unwrap());
    assert_eq!((a, b), (100, 200));

    // Job id=20: a=1000, b=2000
    let (a, b) = decode_u64_pair(r[2].value.as_deref().unwrap());
    assert_eq!((a, b), (1000, 2000));

    // Job id=30: a=42, b=99
    let (a, b) = decode_u64_pair(r[3].value.as_deref().unwrap());
    assert_eq!((a, b), (42, 99));

    // Fr(999) not found
    assert!(!r[4].found);

    eprintln!("All jobs decoded: id=10→(100,200), id=20→(1000,2000), id=30→(42,99), id=999→not_found");
}
