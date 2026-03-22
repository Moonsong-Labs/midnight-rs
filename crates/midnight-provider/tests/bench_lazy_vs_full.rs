//! Benchmark: full blob (`midnight_contractState`) vs lazy query (`midnight_queryContractState`)
//!
//! Measures the actual Rust client path including WebSocket RPC, deserialization, and value access.
//!
//! Run: cargo test --test bench_lazy_vs_full -- --ignored --show-output

use std::time::Instant;

use midnight_bindgen::{
    AlignedValue, InMemoryDB, StateValue,
    cell_value, tagged_deserialize,
};
use midnight_provider::{MidnightProvider, Provider, StateQuery};

// map_stress contract: Map<Field, Bytes<32>> at [0], Counter at [1]
// Deployed with 5000 entries
const CONTRACT: &str = "c8c5dcb9e3a1babcd6079309cb3913adf7ea51ed785dc52e0bc9a00b5d50438c";

fn provider() -> MidnightProvider {
    MidnightProvider::new("ws://127.0.0.1:9944", "http://127.0.0.1:8088").unwrap()
}

#[tokio::test]
#[ignore]
async fn bench_full_blob_vs_lazy_query() {
    let p = provider();
    let iterations = 5;

    println!("\n=== Benchmark: Full Blob vs Lazy Query (Rust client, WebSocket RPC) ===\n");

    // --- Full blob path: get_contract_state → hex decode → tagged_deserialize → navigate ---
    println!("1. Full blob (get_contract_state → deserialize entire state → read counter):");
    let mut full_times = Vec::new();
    let mut blob_size = 0;
    for i in 0..iterations {
        let start = Instant::now();

        // This is what contract.ledger() does internally
        let hex = p.get_contract_state(CONTRACT).await.unwrap().unwrap();
        blob_size = hex.len() / 2;
        let bytes = midnight_bindgen::hex::decode(&hex).unwrap();
        let contract_state: midnight_bindgen::ContractState<InMemoryDB> =
            tagged_deserialize(&mut bytes.as_slice()).unwrap();
        let root: &StateValue<InMemoryDB> = contract_state.data.get_ref();

        // Navigate to counter (field [1])
        let counter_sv = match root {
            StateValue::Array(arr) => arr.get(1).unwrap(),
            _ => panic!("expected array"),
        };
        let counter_av = cell_value(counter_sv).unwrap();
        let counter = u64::try_from(&*counter_av.value).unwrap();

        let elapsed = start.elapsed();
        full_times.push(elapsed.as_secs_f64() * 1000.0);

        if i == 0 {
            println!("   Counter = {counter}, blob = {blob_size} bytes");
        }
    }

    // --- Lazy query path: query_contract_state → small response → deserialize value ---
    println!("\n2. Lazy query (query_contract_state → deserialize single value):");
    let mut lazy_counter_times = Vec::new();
    for i in 0..iterations {
        let start = Instant::now();

        let results = p.query_contract_state(CONTRACT, vec![
            StateQuery { field_path: vec![1], key: None },
        ]).await.unwrap();
        let hex = results[0].value.as_deref().unwrap();
        let bytes = midnight_bindgen::hex::decode(hex).unwrap();
        let sv: StateValue<InMemoryDB> = tagged_deserialize(&mut bytes.as_slice()).unwrap();
        let counter_av = cell_value(&sv).unwrap();
        let counter = u64::try_from(&*counter_av.value).unwrap();

        let elapsed = start.elapsed();
        lazy_counter_times.push(elapsed.as_secs_f64() * 1000.0);

        if i == 0 {
            println!("   Counter = {counter}, response = {} bytes", hex.len() / 2);
        }
    }

    // --- Lazy query: map key lookup ---
    println!("\n3. Lazy query (single map key lookup → deserialize entry):");
    let mut lazy_map_times = Vec::new();
    for _ in 0..iterations {
        let start = Instant::now();

        let results = p.query_contract_state(CONTRACT, vec![
            StateQuery { field_path: vec![0], key: Some("412a41".into()) }, // Fr(42)
        ]).await.unwrap();
        assert!(results[0].found);

        let elapsed = start.elapsed();
        lazy_map_times.push(elapsed.as_secs_f64() * 1000.0);
    }

    // --- Lazy query: batch (counter + 3 map keys) ---
    println!("\n4. Lazy query batch (counter + 3 map keys in one call):");
    let mut lazy_batch_times = Vec::new();
    for _ in 0..iterations {
        let start = Instant::now();

        let results = p.query_contract_state(CONTRACT, vec![
            StateQuery { field_path: vec![1], key: None },
            StateQuery { field_path: vec![0], key: Some("4041".into()) },   // Fr(0)
            StateQuery { field_path: vec![0], key: Some("412a41".into()) }, // Fr(42)
            StateQuery { field_path: vec![0], key: Some("416341".into()) }, // Fr(99)
        ]).await.unwrap();
        assert_eq!(results.len(), 4);

        let elapsed = start.elapsed();
        lazy_batch_times.push(elapsed.as_secs_f64() * 1000.0);
    }

    // --- Summary ---
    let avg = |times: &[f64]| times.iter().sum::<f64>() / times.len() as f64;

    println!("\n=== Results ({iterations} iterations, {blob_size} byte state blob) ===\n");
    println!("  Full blob (download + deserialize all):  {:.1}ms avg", avg(&full_times));
    println!("  Lazy query (counter):                    {:.1}ms avg", avg(&lazy_counter_times));
    println!("  Lazy query (single map key):             {:.1}ms avg", avg(&lazy_map_times));
    println!("  Lazy query (batch: counter + 3 keys):    {:.1}ms avg", avg(&lazy_batch_times));
    println!();
    println!("  Speedup (counter):    {:.0}x", avg(&full_times) / avg(&lazy_counter_times));
    println!("  Speedup (map key):    {:.0}x", avg(&full_times) / avg(&lazy_map_times));
    println!("  Speedup (batch):      {:.0}x", avg(&full_times) / avg(&lazy_batch_times));
}
