//! Counter contract example.
//!
//! Demonstrates the full lifecycle of a Midnight smart contract:
//! 1. Deploy the counter contract locally
//! 2. Read the initial state through typed accessors
//! 3. Increment 3 times using the generated circuit method
//! 4. Read the final state
//!
//! # Running
//!
//! ```bash
//! MIDNIGHT_LEDGER_TEST_STATIC_DIR=/tmp cargo run --example counter -p midnight-contract
//! ```

use midnight_contract::{deploy_local, format_address};

// Generate typed bindings from the compiled counter contract.
// A single contract-info.json provides:
//   - Ledger struct with typed .round() accessor (from ledger type annotations)
//   - LedgerInitialState struct for deployment (from ledger field definitions)
//   - call_increment() method (from embedded circuit IR)
mod counter {
    midnight_bindgen::contract!("examples/counter-compiled/compiler/contract-info.json");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Counter Example ===\n");

    // Step 1: Build initial state using the generated InitialState
    println!("1. Building initial state...");
    let initial = counter::LedgerInitialState { round: 0 };
    let state = initial.build();
    println!("   round = 0");

    // Step 2: Deploy locally
    println!("\n2. Deploying contract...");
    let (address, test_state) = deploy_local(&state).await?;
    println!("   Address: {}", format_address(&address));
    println!(
        "   In ledger: {}",
        test_state.ledger.contract.get(&address).is_some()
    );

    // Step 3: Read initial state
    println!("\n3. Reading initial state...");
    let ledger = counter::Ledger::new(state);
    println!("   round = {}", ledger.round()?);

    // Step 4: Increment 3 times
    println!("\n4. Incrementing...");
    let ledger = ledger.call_increment()?;
    println!("   round = {}", ledger.round()?);
    let ledger = ledger.call_increment()?;
    println!("   round = {}", ledger.round()?);
    let ledger = ledger.call_increment()?;
    println!("   round = {}", ledger.round()?);

    assert_eq!(ledger.round()?, 3);
    println!("\n5. Final: round = {} ✓", ledger.round()?);

    println!("\n=== Done ===");
    Ok(())
}
