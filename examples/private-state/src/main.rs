//! Per-contract private state, end to end.
//!
//! A contract's **private state** is off-chain data its stateful witnesses read
//! and write between calls. It never touches the chain, but it must survive
//! across calls. This example deploys a contract whose circuit calls a witness,
//! and shows the SDK's load → witness → persist loop doing real work: with a
//! `PrivateStateProvider` attached, a circuit call loads the contract's private
//! state, hands it to the witness, and persists whatever the witness wrote.
//!
//! The `secret-counter` contract (see `devnet/contracts/secret-counter`):
//!
//! ```compact
//! export ledger total: Counter;
//! witness next_secret(): Uint<16>;          // value comes from private state
//! export circuit contribute(): Uint<16> {
//!   const s = next_secret();
//!   total.increment(disclose(s));           // fold the secret into public total
//!   return disclose(s);
//! }
//! ```
//!
//! Our witness keeps a private running counter *in the private state*: each
//! `next_secret()` returns the next value (1, 2, 3, …) and advances the stored
//! counter. So calling `contribute()` twice returns 1 then 2 — the second call
//! only knows to return 2 because the first call's `1` was persisted and
//! reloaded. The chain sees only the disclosed contributions (`total` = 1, then
//! 3); the running counter stays off-chain.
//!
//! Runs against the shared local devnet (`devnet/docker-compose.yml`); see
//! README.md. The private-state store also supports password-encrypted
//! export/import for backup — see `docs/private-state.md`.

use std::sync::Arc;

use midnight_contract::interpreter::{self, Value, WitnessContext, WitnessProvider};
use midnight_provider::{
    FsPrivateStateProvider, MidnightProvider, Network, PrivateStateProvider, WalletSeed,
};

mod secret_counter {
    midnight_bindgen::contract!(
        "../../devnet/contracts/secret-counter/compiled/contract-info.json"
    );
}

const NODE_URL: &str = "ws://127.0.0.1:9944";
const INDEXER_URL: &str = "http://127.0.0.1:8088";
const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/secret-counter/compiled"
);

/// Dev node genesis wallet seed (funded with NIGHT tokens at genesis).
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// The off-chain half of the contract's `next_secret` witness.
///
/// The witness value comes entirely from the contract's private state, which
/// the SDK loads into `ctx` before the call and persists after. This witness
/// reads the running counter from `ctx`, returns the next value, and writes the
/// advanced counter back — it owns the byte encoding (here, a little-endian
/// `u64`). It never touches storage directly; the SDK does the load/persist.
struct SecretWitness;

fn decode_counter(bytes: &[u8]) -> u64 {
    bytes.try_into().map(u64::from_le_bytes).unwrap_or(0) // empty = fresh contract, counter starts at 0
}

impl WitnessProvider for SecretWitness {
    fn call_witness(
        &self,
        ctx: &mut WitnessContext<'_>,
        name: &str,
        _args: &[Value],
    ) -> Result<Value, interpreter::InterpreterError> {
        match name {
            "next_secret" => {
                let next = decode_counter(ctx.private_state()) + 1;
                ctx.set_private_state(next.to_le_bytes().to_vec());
                Ok(Value::Integer(next as u128))
            }
            other => Err(interpreter::InterpreterError::Witness(format!(
                "unknown witness: {other}"
            ))),
        }
    }
}

/// Read the persisted private-state counter for a contract straight from the
/// store (what survives across calls and restarts).
async fn stored_counter(
    store: &Arc<dyn PrivateStateProvider>,
    address: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(store
        .get(address)
        .await?
        .map(|b| decode_counter(&b))
        .unwrap_or(0))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Midnight Private State Example ===\n");

    // A durable, contract-scoped private-state store. A real app would use
    // `FsPrivateStateProvider::with_default_dir()` (`~/.midnight/private-state/`);
    // a temp dir keeps the example repeatable.
    let dir = std::env::temp_dir().join("midnight-private-state-example");
    let _ = std::fs::remove_dir_all(&dir);
    let store: Arc<dyn PrivateStateProvider> = Arc::new(FsPrivateStateProvider::new(&dir));

    println!("0. Syncing wallet and attaching the private-state store...");
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED)?;
    let provider = MidnightProvider::new(NODE_URL, INDEXER_URL)?
        .with_private_state(store.clone())
        .sync_wallet(seed, Network::Undeployed)
        .await?;
    println!("   synced.\n");

    // 1. Deploy the secret-counter contract.
    println!("1. Deploying secret-counter...");
    let pending = secret_counter::Contract::deploy(&provider)
        .with_initial_state(secret_counter::LedgerInitialState::default())
        .with_zk_keys(ZK_KEYS_DIR)
        .send()
        .await?;
    let (_, pending) = pending.wait_best().await?;
    let contract = pending.into_contract().await?;
    let address = contract.address().to_string();
    println!("   address: {address}");
    println!(
        "   on-chain total = {}, private counter = {}\n",
        contract.ledger().await?.total()?,
        stored_counter(&store, &address).await?
    );

    // 2 & 3. Call `contribute()` twice. The witness supplies the next secret
    //        from the private state; the SDK persists the advance after each
    //        call, so call #2 loads what call #1 wrote.
    for call in 1..=2 {
        println!("{}. Calling contribute()...", call + 1);
        let returned: u16 = contract
            .circuits()
            .with_witnesses(&SecretWitness)
            .contribute()
            .await?;
        println!(
            "   witness disclosed {returned}; on-chain total = {}, persisted private counter = {}",
            contract.ledger().await?.total()?,
            stored_counter(&store, &address).await?
        );
    }

    println!(
        "\nThe private counter advanced 0 → 1 → 2 across calls (loaded and persisted\n\
         each time), while the chain only saw the disclosed contributions in `total`\n\
         (1, then 3). The counter is on disk under {}, so it survives restarts.",
        dir.display()
    );
    println!("\n=== Done ===");
    Ok(())
}
