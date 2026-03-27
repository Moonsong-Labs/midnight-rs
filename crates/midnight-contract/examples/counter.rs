//! Counter contract example.
//!
//! Demonstrates the full lifecycle of a Midnight smart contract using midnight-rs:
//! 1. Deploy the counter contract locally
//! 2. Read the initial state through typed accessors
//! 3. Increment 3 times using generated circuit methods
//! 4. Read the final state
//!
//! # Running
//!
//! ```bash
//! MIDNIGHT_LEDGER_TEST_STATIC_DIR=/tmp cargo run --example counter -p midnight-contract
//! ```
//!
//! # With a devnet (Docker)
//!
//! ```bash
//! cd examples/counter
//! docker compose up -d
//! # Wait for health: until curl -sf http://localhost:9944/health; do sleep 2; done
//! # Then deploy to node using deploy_with_provider + submit
//! docker compose down
//! ```

use midnight_contract::{deploy_local, format_address};

// Typed accessors (standard compiler output — has type annotations)
mod counter_typed {
    midnight_bindgen::contract!("examples/counter-compiled/compiler/contract-info-typed.json");
}

// Circuit call methods (fork compiler output — has embedded IR)
mod counter_ir {
    midnight_bindgen::contract!("examples/counter-compiled/compiler/contract-info.json");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Counter Example ===\n");

    // ---------------------------------------------------------------
    // Step 1: Build initial state
    // ---------------------------------------------------------------
    println!("1. Building initial state...");
    let initial = counter_typed::LedgerInitialState { round: 0 };
    let state = initial.build();
    println!("   Created ContractState with round = 0");

    // ---------------------------------------------------------------
    // Step 2: Deploy locally (uses midnight-ledger TestState)
    // ---------------------------------------------------------------
    println!("\n2. Deploying counter contract...");
    let (address, test_state) = deploy_local(&state).await?;
    println!("   Address: {}", format_address(&address));
    println!(
        "   Verified in ledger: {}",
        test_state.ledger.contract.get(&address).is_some()
    );

    // ---------------------------------------------------------------
    // Step 3: Read initial state through typed accessor
    // ---------------------------------------------------------------
    println!("\n3. Reading initial state...");
    let ledger = counter_typed::Ledger::new(state);
    println!("   round = {}", ledger.round()?);

    // ---------------------------------------------------------------
    // Step 4: Increment 3 times using generated circuit method
    // ---------------------------------------------------------------
    println!("\n4. Incrementing 3 times...");

    // Use the IR-containing module for circuit calls
    let ir_ledger = counter_ir::Ledger::new(ledger.into_contract_state());

    let ir_ledger = ir_ledger.call_increment()?;
    // Read back through typed accessor
    let typed = counter_typed::Ledger::new(ir_ledger.into_contract_state());
    println!("   After increment 1: round = {}", typed.round()?);

    let ir_ledger = counter_ir::Ledger::new(typed.into_contract_state());
    let ir_ledger = ir_ledger.call_increment()?;
    let typed = counter_typed::Ledger::new(ir_ledger.into_contract_state());
    println!("   After increment 2: round = {}", typed.round()?);

    let ir_ledger = counter_ir::Ledger::new(typed.into_contract_state());
    let ir_ledger = ir_ledger.call_increment()?;
    let typed = counter_typed::Ledger::new(ir_ledger.into_contract_state());
    println!("   After increment 3: round = {}", typed.round()?);

    assert_eq!(typed.round()?, 3);

    // ---------------------------------------------------------------
    // Step 5: Summary
    // ---------------------------------------------------------------
    println!("\n5. Final state: round = {} ✓", typed.round()?);

    println!("\n--- To deploy on a real node ---");
    println!(
        "  let provider = MidnightProvider::new(\"ws://localhost:9944\", \"http://localhost:8088\")?;"
    );
    println!("  let (addr, tx) = deploy_with_provider(&provider, &state).await?;");
    println!("  submit(\"ws://localhost:9944\", &tx).await?;");

    println!("\n=== Done ===");
    Ok(())
}
