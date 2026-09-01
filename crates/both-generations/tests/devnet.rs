//! Against a running devnet: reach a contract without naming a generation.
//!
//! Deploying is covered too: the client attaches a wallet from a seed, and
//! the generation is never named.
//!
//! Skips unless `MIDNIGHT_E2E` is set, like the rest of the devnet suite.

use both_generations::bindings::counter::CounterDispatch;
use midnight_dispatch::{ArgValue, CircuitCall, Client, Opening, OpeningField};

#[tokio::test]
async fn connects_and_reads_without_naming_a_generation() {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    }
    let node = std::env::var("MIDNIGHT_NODE_URL").expect("MIDNIGHT_NODE_URL");
    let indexer = std::env::var("MIDNIGHT_INDEXER_URL").expect("MIDNIGHT_INDEXER_URL");

    // Nothing below names a ledger generation.
    let client = Client::connect(&node, &indexer).await.expect("connect");
    let generation = client.generation();
    eprintln!("the chain runs {generation}");

    let state = client
        .contract_state("0200aa1d5e0a90b2a0e1c6bbd0b1e8f4a0d5c3b2a1908f7e6d5c4b3a2918071620")
        .await
        .expect("state query answered");

    // The address is a placeholder, so the indexer reports no such contract.
    // What matters is that the query dispatched and the reader parses for
    // whichever generation the chain runs.
    match state {
        Some(hex) => {
            let counter = CounterDispatch::from_hex(generation, &hex).expect("parse");
            eprintln!("round = {:?}", counter.round());
        }
        None => eprintln!("no contract at the placeholder address, as expected"),
    }
}

/// Deploy a counter and read its opening value back, naming no generation.
#[tokio::test]
async fn deploys_and_reads_back_without_naming_a_generation() {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    }
    let node = std::env::var("MIDNIGHT_NODE_URL").expect("MIDNIGHT_NODE_URL");
    let indexer = std::env::var("MIDNIGHT_INDEXER_URL").expect("MIDNIGHT_INDEXER_URL");
    let mut seed = [0u8; 32];
    seed[31] = 1;

    let client = Client::connect(&node, &indexer)
        .await
        .expect("connect")
        .with_wallet(&indexer, seed, "undeployed")
        .await
        .expect("attach a wallet");
    let generation = client.generation();

    // Relative to this crate, not the workspace root: a test runs with its
    // own manifest directory as the working directory.
    let compiled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../devnet/contracts/counter/compiled");
    let address = client
        .deploy(
            compiled.to_str().expect("utf-8 path"),
            Opening::new(vec![OpeningField::Counter(0)]),
        )
        .await
        .expect("deploy");
    eprintln!("deployed to {address} on {generation}");

    let hex = client
        .contract_state(&address)
        .await
        .expect("state query")
        .expect("the contract exists");
    let counter = CounterDispatch::from_hex(generation, &hex).expect("parse state");
    assert_eq!(
        counter.round().expect("round"),
        0,
        "a fresh counter opens at 0"
    );
}

/// Call a circuit with an argument, naming no generation. `increment_by` takes
/// one, so this exercises the neutral argument type rather than only the
/// no-argument path.
#[tokio::test]
async fn calls_a_circuit_without_naming_a_generation() {
    if std::env::var("MIDNIGHT_E2E").is_err() {
        eprintln!("skipped: set MIDNIGHT_E2E");
        return;
    }
    let node = std::env::var("MIDNIGHT_NODE_URL").expect("MIDNIGHT_NODE_URL");
    let indexer = std::env::var("MIDNIGHT_INDEXER_URL").expect("MIDNIGHT_INDEXER_URL");
    let mut seed = [0u8; 32];
    seed[31] = 1;

    let client = Client::connect(&node, &indexer)
        .await
        .expect("connect")
        .with_wallet(&indexer, seed, "undeployed")
        .await
        .expect("attach a wallet");
    let generation = client.generation();
    let compiled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../devnet/contracts/counter/compiled");
    let compiled = compiled.to_str().expect("utf-8 path");

    let address = client
        .deploy(compiled, Opening::new(vec![OpeningField::Counter(0)]))
        .await
        .expect("deploy");

    let sexp = std::fs::read_to_string(std::path::Path::new(compiled).join("analyzed-ir.sexp"))
        .expect("read the compiled artifact");
    let info = compact_codegen::artifact::load_str(&sexp).expect("load the artifact");
    // `ContractInfo` wraps each circuit with its exported name; the interpreter
    // wants the definitions.
    let circuits: Vec<_> = info
        .circuits
        .iter()
        .map(|c| c.def.clone())
        .chain(info.helpers.iter().cloned())
        .collect();
    let circuit = info
        .circuits
        .iter()
        .find(|c| c.name == "increment_by")
        .map(|c| c.def.clone())
        .expect("the counter exports increment_by");
    client
        .call(CircuitCall {
            address: &address,
            zk_config_dir: compiled,
            circuit: &circuit,
            circuits: &circuits,
            witnesses: &info.witnesses,
            natives: &info.natives,
            circuit_name: "increment_by",
            args: &[("amount", ArgValue::Integer(5))],
        })
        .await
        .expect("call increment");

    // The call landed on chain; the indexer serves state a moment behind, so
    // poll rather than read once.
    let mut round = 0u64;
    for _ in 0..20 {
        let hex = client
            .contract_state(&address)
            .await
            .expect("state query")
            .expect("the contract exists");
        round = CounterDispatch::from_hex(generation, &hex)
            .expect("parse state")
            .round()
            .expect("round");
        if round == 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert_eq!(
        round, 5,
        "increment_by(5) should advance the counter by its argument"
    );
}
