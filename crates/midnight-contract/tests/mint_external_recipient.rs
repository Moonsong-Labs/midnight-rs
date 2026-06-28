//! Live devnet E2E for full shielded mint.
//!
//! Mints a shielded coin to an EXTERNAL recipient (a second wallet) via the
//! `mint` circuit + per-call coin encryption keys, then asserts the recipient's
//! wallet discovers the coin through normal sync, no `watchFor`.
//!
//! Gated on `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` (a running devnet +
//! indexer). The compiled contract defaults to the committed fixture
//! `devnet/contracts/shielded-mint/compiled`; override with `MINT_KEYED_DIR`.

use midnight_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, StateValue, StorageHashMap,
};
use midnight_contract::Contract;
use midnight_contract::interpreter::{self, Value};

#[tokio::test]
async fn mint_to_external_recipient_discovered_by_sync() {
    let (node_url, indexer_url) = match (
        std::env::var("MIDNIGHT_NODE_URL").ok(),
        std::env::var("MIDNIGHT_INDEXER_URL").ok(),
    ) {
        (Some(n), Some(i)) => (n, i),
        _ => {
            eprintln!("skipping: needs MIDNIGHT_NODE_URL + MIDNIGHT_INDEXER_URL");
            return;
        }
    };
    let keyed = std::env::var("MINT_KEYED_DIR").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../devnet/contracts/shielded-mint/compiled"
        )
        .to_string()
    });

    // --- Load the mint circuit IR, helpers, structs, enums, arg-types ---
    let info_path = format!("{keyed}/contract-info.json");
    let info_json = std::fs::read_to_string(&info_path).expect("read contract-info");
    let info: compact_codegen::types::ContractInfo =
        serde_json::from_str(&info_json).expect("parse contract-info");
    let mint = info
        .circuits
        .iter()
        .find(|c| c.name == "mint")
        .expect("mint circuit");
    let ir: compact_codegen::ir::CircuitIrBody =
        serde_json::from_value(serde_json::to_value(mint.ir.as_ref().expect("mint IR")).unwrap())
            .unwrap();

    let helpers = &info.helpers;
    let mut structs = info.structs.clone();
    let mut enums: Vec<compact_codegen::ir::EnumDef> = Vec::new();
    compact_codegen::arg_types::collect_argument_defs(&mint.arguments, &mut structs, &mut enums);
    let arg_types_owned = compact_codegen::arg_types::circuit_arg_types(&mint.arguments);
    let arg_types: Vec<(&str, compact_codegen::ir::TypeRef)> = arg_types_owned
        .iter()
        .map(|(n, t)| (n.as_str(), t.clone()))
        .collect();

    // --- Recipient (cpk, epk) from a second seed (derived, never synced for
    //     the mint itself) ---
    let recip_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000002",
    )
    .unwrap();
    let recip_addr = midnight_wallet::address::derive_shielded(
        &recip_seed,
        midnight_provider::Network::Undeployed,
    );
    let recip = midnight_wallet::transfer::parse_shielded_recipient(&recip_addr)
        .expect("parse recipient address");
    let cpk = recip.coin_public_key;
    let epk = recip.enc_public_key;
    let cpk_bytes = cpk.0.0;

    // --- Funder wallet deploys the mint contract ---
    let funder_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(funder_seed, midnight_provider::Network::Undeployed)
        .await
        .expect("funder sync");

    // A mint-only contract has no user ledger fields: an empty array.
    let initial = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let contract = Contract::deploy(provider)
        .with_initial_state(initial)
        .with_zk_keys(&keyed)
        .await
        .expect("deploy mint contract");
    let address = contract.address().to_string();
    eprintln!("deployed mint contract at {address}");

    // --- Call mint(domain_sep, value, nonce, coinPK) ---
    //
    // The contract wraps the recipient with a compile-time-constant
    // `left<ZswapCoinPublicKey, ContractAddress>(coinPK)`, so `coinPK` is a plain
    // `ZswapCoinPublicKey` struct (a single `Bytes<32>`), not a runtime `Either`.
    // A runtime `Either` recipient makes the `mintShieldedToken` builtin compile to
    // a fixed 80-public-input circuit that carries the (untaken) contract-recipient
    // branch; its skipped public inputs are non-zero (recipient/commitment data) and
    // can't be reproduced by the prover's zero-noop padding, which fails SNARK verify
    // with InvalidProof. The constant `left(...)` folds that branch away.
    let domain_sep = [0x11u8; 32];
    let mint_value: u128 = 1000;
    let nonce = [0x22u8; 32];
    let args = [
        (
            "domain_sep",
            Value::AlignedValue(AlignedValue::from(domain_sep)),
        ),
        ("value", Value::Integer(mint_value)),
        ("nonce", Value::AlignedValue(AlignedValue::from(nonce))),
        ("coinPK", Value::AlignedValue(AlignedValue::from(cpk_bytes))),
    ];

    // The coin→enc key mapping is supplied per-call so the SDK attaches the
    // discovery ciphertext to the circuit-created output.
    contract
        .call_with(
            &ir,
            "mint",
            &args,
            &interpreter::NoWitnesses,
            midnight_contract::CircuitDefs {
                arg_types: &arg_types,
                helpers,
                structs: &structs,
                enums: &enums,
            },
            &[(cpk, epk)],
        )
        .await
        .expect("mint call");
    eprintln!("mint call submitted");

    // Expected custom shielded token type = tokenType(domain_sep, contract_addr).
    // The contract address is a 32-byte hex string.
    let addr_bytes = {
        let hex = address.strip_prefix("0x").unwrap_or(&address);
        let v = hex::decode(hex).expect("address hex");
        let mut a = [0u8; 32];
        a.copy_from_slice(&v);
        a
    };
    let contract_addr = midnight_coin_structure::contract::ContractAddress(
        midnight_base_crypto::hash::HashOutput(addr_bytes),
    );
    let expected_tt = contract_addr
        .custom_shielded_token_type(midnight_base_crypto::hash::HashOutput(domain_sep));

    // --- Recipient syncs normally and must discover the minted coin ---
    let recip_provider = midnight_provider::MidnightProvider::new(&node_url, &indexer_url)
        .expect("provider")
        .sync_wallet(recip_seed, midnight_provider::Network::Undeployed)
        .await
        .expect("recipient sync");
    let balance = recip_provider.balance().await.expect("recipient balance");

    let found = balance
        .shielded
        .coins
        .iter()
        .any(|c| c.token_type == expected_tt && c.value == mint_value);
    assert!(
        found,
        "recipient wallet did not discover the minted coin (type {}, value {mint_value}); \
         shielded coins seen: {:?}",
        hex::encode(expected_tt.0.0),
        balance
            .shielded
            .coins
            .iter()
            .map(|c| (hex::encode(c.token_type.0.0), c.value))
            .collect::<Vec<_>>()
    );
    eprintln!("recipient discovered the minted coin via sync ✓");
}
