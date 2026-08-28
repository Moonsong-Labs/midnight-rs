//! Live devnet E2E for recovering a coin the wallet cannot discover.
//!
//! Mints two shielded coins to a second wallet through the `mint` circuit,
//! with no coin encryption keys attached, so neither output carries a
//! discovery ciphertext. The recipient's sync cannot see them: it owns them,
//! but the only thing on chain that names each one is its commitment.
//! Rebuilding a coin from what the caller knows (nonce, token type, value)
//! and registering it with `watch_for_coin` recovers it.
//!
//! The second coin is what proves recovery composes: registering it replays
//! the stream again, and the first coin has to survive that replay even
//! though the ledger consumed its registration when it claimed it.
//!
//! This is the bridge-gateway case: a contract mints to a coin public key
//! whose encryption key the minter does not have, and evolves one public
//! nonce per mint, so the owner can rebuild the coin from chain state.
//!
//! Gated on `MIDNIGHT_NODE_URL` / `MIDNIGHT_INDEXER_URL` (a running devnet +
//! indexer). The compiled contract defaults to the committed fixture
//! `devnet/contracts/shielded-mint/compiled`; override with `MINT_KEYED_DIR`.

use compact_bindgen::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, StateValue, StorageHashMap,
};
use midnight_contract::Contract;
use midnight_contract::runtime::Value;
use midnight_wallet::{LocalWallet, Wallet};

#[tokio::test]
async fn a_coin_with_no_ciphertext_is_recovered_by_registering_it() {
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
    let info_path = format!("{keyed}/analyzed-ir.sexp");
    let info_json = std::fs::read_to_string(&info_path).expect("read contract-info");
    let info: compact_codegen::types::ContractInfo =
        compact_codegen::artifact::load_str(&info_json).expect("parse contract-info");
    let mint = info
        .circuits
        .iter()
        .find(|c| c.name == "mint")
        .expect("mint circuit");
    let ir = &mint.def;
    let program =
        midnight_contract::interpreter::Program::new(&info.helpers, &info.witnesses, &info.natives);

    // --- The recipient's coin public key. Its encryption key is deliberately
    //     never handed to the call, so the mint's output carries no
    //     ciphertext ---
    let recip_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000002",
    )
    .unwrap();
    let recip_addr = midnight_types::address::derive_shielded(
        &recip_seed,
        midnight_provider::Network::Undeployed,
    );
    let cpk = midnight_types::transfer::parse_shielded_recipient(
        &recip_addr,
        midnight_provider::Network::Undeployed,
    )
    .expect("parse recipient address")
    .coin_public_key;

    // --- Funder wallet deploys the mint contract ---
    let funder_seed = midnight_provider::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap();
    let provider =
        midnight_provider::MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        funder_seed,
        midnight_provider::Network::Undeployed,
    )
    .await
    .expect("funder sync");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    // A mint-only contract has no user ledger fields: an empty array.
    let initial = ContractState::new(
        StateValue::Array(vec![].into()),
        StorageHashMap::new(),
        ContractMaintenanceAuthority::default(),
    );

    let contract = Contract::deploy(provider)
        .with_initial_state(initial)
        .with_zk_config(&keyed)
        .await
        .expect("deploy mint contract");
    let address = contract.address().to_string();
    eprintln!("deployed mint contract at {address}");

    // --- Call mint(domain_sep, value, nonce, coinPK) with no encryption keys,
    //     once per coin. See `mint_external_recipient` for why the recipient
    //     is a constant `left(...)` in the contract ---
    let domain_sep = [0x11u8; 32];
    let mints: [([u8; 32], u128); 2] = [([0x33u8; 32], 1000), ([0x44u8; 32], 250)];

    for (nonce, mint_value) in mints {
        let args = [
            (
                "domain_sep",
                Value::AlignedValue(AlignedValue::from(domain_sep)),
            ),
            ("value", Value::Integer(mint_value)),
            ("nonce", Value::AlignedValue(AlignedValue::from(nonce))),
            ("coinPK", Value::AlignedValue(AlignedValue::from(cpk.0.0))),
        ];

        contract
            .call_with(
                ir,
                &program,
                "mint",
                &args,
                &midnight_contract::runtime::NoWitnesses,
                &[],
                midnight_contract::ShieldedInputs::default(),
            )
            .await
            .expect("mint call");
        eprintln!("minted {mint_value} with no discovery ciphertext");
    }

    // The coin the recipient has to rebuild: the nonce and value the call
    // used, and the token type the contract mints, `tokenType(domain_sep,
    // contract_addr)`. The contract address is a 32-byte hex string.
    let addr_bytes = {
        let hex = address.strip_prefix("0x").unwrap_or(&address);
        let v = hex::decode(hex).expect("address hex");
        let mut a = [0u8; 32];
        a.copy_from_slice(&v);
        a
    };
    let contract_addr = midnight_helpers::coin_structure::contract::ContractAddress(
        midnight_base_crypto::hash::HashOutput(addr_bytes),
    );
    let token_type = contract_addr
        .custom_shielded_token_type(midnight_base_crypto::hash::HashOutput(domain_sep));

    // --- The recipient syncs and cannot see either coin ---
    let recip_provider =
        midnight_provider::MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    let wallet = Wallet::sync(
        recip_provider.indexer_url(),
        recip_seed,
        midnight_provider::Network::Undeployed,
    )
    .await
    .expect("recipient sync");
    let recip_provider = recip_provider.with_wallet(LocalWallet::new(wallet));

    let holds = |coins: &[midnight_provider::SpendableShieldedCoin], nonce: [u8; 32], value| {
        coins
            .iter()
            .any(|c| c.token_type == token_type && c.value == value && c.nonce == nonce)
    };
    let seen = |coins: &[midnight_provider::SpendableShieldedCoin]| {
        coins
            .iter()
            .map(|c| (hex::encode(c.token_type.0.0), c.value))
            .collect::<Vec<_>>()
    };

    let before = recip_provider
        .spendable_shielded_coins()
        .await
        .expect("recipient coins");
    for (nonce, value) in mints {
        assert!(
            !holds(&before, nonce, value),
            "a coin with no ciphertext must not be discoverable by sync alone"
        );
    }

    // --- Register each rebuilt coin in turn: the wallet claims it from the
    //     chain's own output, decrypting nothing. Registering the second must
    //     not cost us the first ---
    for (index, (nonce, value)) in mints.iter().copied().enumerate() {
        recip_provider
            .watch_for_coin(midnight_provider::CoinInfo {
                nonce: midnight_provider::Nonce(midnight_base_crypto::hash::HashOutput(nonce)),
                type_: token_type,
                value,
            })
            .await
            .expect("watch_for_coin");

        let after = recip_provider
            .spendable_shielded_coins()
            .await
            .expect("recipient coins");
        for (recovered_nonce, recovered_value) in mints.iter().copied().take(index + 1) {
            assert!(
                holds(&after, recovered_nonce, recovered_value),
                "registering the rebuilt coin must recover it and keep the ones before it \
                 (token {}, value {recovered_value}); coins seen: {:?}",
                hex::encode(token_type.0.0),
                seen(&after)
            );
        }
    }
    eprintln!("recipient recovered both coins through watch_for_coin ✓");
}
