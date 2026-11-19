//! A contract deploys in two waves: the deploy registers some of its
//! circuits, and one maintenance update inserts the others and freezes the
//! contract. A circuit of the second wave then takes calls.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use midnight_contract::{
    ContractMaintenanceAuthority, ContractMaintenanceSigningKey, ContractMaintenanceVerifyingKey,
    FsZkConfigProvider, ZkConfigProvider, schnorr,
};
use midnight_onchain_runtime::state::EntryPointBuf;
use midnight_provider::MidnightProvider;

/// A deploy that drops the registered set carries every key. An update that
/// cannot insert a key and freeze together leaves a contract either governed
/// or short of a circuit.
#[tokio::test]
async fn a_contract_deploys_in_two_waves_and_freezes() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };
    assert!(
        std::path::Path::new(counter::ZK_CONFIG_DIR)
            .join("keys")
            .is_dir(),
        "{}/keys is missing; run `make compile-contracts CONTRACTS=counter`",
        counter::ZK_CONFIG_DIR
    );

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let provider = provider.with_wallet(wallet);

    let key = schnorr::SigningKey::sample(rand::thread_rng());
    let member = ContractMaintenanceVerifyingKey::Schnorr(key.verifying_key());
    let authority = ContractMaintenanceSigningKey::Schnorr(key);

    let contract = counter::Contract::deploy(&provider)
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .with_maintenance_authority(vec![member], 1)
        .with_registered_circuits(["increment"])
        .send()
        .await
        .expect("submit the first wave")
        .into_contract()
        .await
        .unwrap_or_else(|e| panic!("the first wave must apply: {e:?}"));

    let increment: EntryPointBuf = b"increment"[..].into();
    let increment_by: EntryPointBuf = b"increment_by"[..].into();
    let first_wave = midnight_contract::state::fetch_state(&provider, contract.address(), None)
        .await
        .expect("read the state after the first wave");
    assert!(first_wave.operations.contains_key(&increment));
    assert!(
        !first_wave.operations.contains_key(&increment_by),
        "the deploy must leave increment_by for the second wave"
    );

    let increment_by_key = FsZkConfigProvider::new(counter::ZK_CONFIG_DIR)
        .verifier_key("increment_by")
        .expect("read the increment_by verifier key");
    contract
        .maintenance()
        .insert_verifier_key("increment_by", increment_by_key)
        .freeze()
        .prepare()
        .await
        .expect("prepare the second wave")
        .sign(0, &authority)
        .send()
        .await
        .expect("submit the second wave")
        .get_receipt()
        .await
        .unwrap_or_else(|e| panic!("the second wave must apply: {e:?}"));

    let second_wave = midnight_contract::state::fetch_state(&provider, contract.address(), None)
        .await
        .expect("read the state after the second wave");
    assert!(second_wave.operations.contains_key(&increment_by));
    assert_eq!(
        second_wave.maintenance_authority,
        ContractMaintenanceAuthority {
            committee: Vec::new(),
            threshold: 1,
            counter: 1,
        },
        "the second wave must freeze the contract"
    );

    // `get_receipt` waits for finality, so the test ends on a final block.
    // The next test in the suite is another process on the same seed, and
    // its wallet reads finalized state only.
    contract
        .circuits()
        .increment_by(5)
        .send()
        .await
        .expect("submit increment_by")
        .get_receipt()
        .await
        .unwrap_or_else(|e| panic!("a circuit of the second wave must take calls: {e:?}"));
}
