//! A chain's hard fork from ledger 8 to ledger 9, and the wallets that cross
//! it. One stays attached across the fork, one resumes a snapshot stored
//! before it, and one syncs from genesis.
//!
//! Needs the fork devnet, which starts on ledger 8. `make fork-up fork-test`
//! runs it: the test forks the chain between its halves with the command in
//! `MIDNIGHT_FORK_UPGRADE_CMD`.

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

use std::path::Path;
use std::time::{Duration, Instant};

use midnight_provider::{
    Builds, LedgerVersion, MidnightProvider, NIGHT, Network, Nullifier, WalletSeed,
};
use midnight_wallet::{LocalWallet, Wallet};

const ZK_KEYS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../devnet/contracts/counter/compiled"
);
const DEV_WALLET_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn a_wallet_crosses_the_fork_to_ledger_9() {
    let (Ok(node_url), Ok(indexer_url)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        eprintln!("skipping: needs the fork devnet (make fork-up fork-test)");
        return;
    };
    let Ok(upgrade) = std::env::var("MIDNIGHT_FORK_UPGRADE_CMD") else {
        if std::env::var_os("MIDNIGHT_E2E").is_some() {
            panic!("MIDNIGHT_FORK_UPGRADE_CMD is missing under make fork-test");
        }
        eprintln!("skipping: MIDNIGHT_FORK_UPGRADE_CMD not set");
        return;
    };
    let seed = WalletSeed::try_from_hex_str(DEV_WALLET_SEED).unwrap();
    let network = Network::Undeployed;
    let provider = MidnightProvider::new(&node_url, &indexer_url).expect("provider");
    assert_eq!(
        provider.ledger_version().await.expect("the chain's ledger"),
        LedgerVersion::V8,
        "the fork devnet must still run ledger 8: recreate it with make fork-down fork-up"
    );

    // Ledger 8: a stored wallet, and a contract it deploys and calls.
    let storage = tempfile::TempDir::new().unwrap();
    let wallet = Wallet::sync(&provider, seed.clone(), network.clone())
        .with_storage(storage.path())
        .await
        .expect("sync on ledger 8");
    let provider = provider.with_wallet(LocalWallet::new(wallet));
    let contract = counter::Contract::deploy(&provider)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(ZK_KEYS_DIR)
        .await
        .expect("deploy on ledger 8");
    contract
        .circuits()
        .increment()
        .await
        .expect("call on ledger 8");
    // The snapshot as it stands on ledger 8, for two resumes after the fork.
    let stored = [(); 2].map(|()| tempfile::TempDir::new().unwrap());
    for dir in &stored {
        copy_dir(storage.path(), dir.path());
    }

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&upgrade)
        .status()
        .expect("run the upgrade command");
    assert!(status.success(), "the upgrade command failed");
    wait_for_ledger_9(&provider).await;

    // A shielded rescan meets ledger 9 events first. It crosses the attached
    // wallet, then replays the shielded stream across the fork.
    provider
        .rescan_shielded()
        .await
        .expect("rescan across the fork");
    assert!(
        matches!(provider.builds().await.expect("builds"), Builds::Ledger9(_)),
        "the attached wallet must cross to ledger 9"
    );

    // The fork emptied the Dust state and its registrations, so no NIGHT
    // generates Dust. A wallet that resumes its ledger 8 snapshot agrees, and
    // so does a sync from genesis, which reads the indexer's flags.
    let unregistered = provider
        .balance()
        .await
        .unwrap()
        .dust
        .unregistered_night_utxos;
    assert!(unregistered > 0, "no NIGHT generates Dust after the fork");
    let resumed = Wallet::sync(&provider, seed.clone(), network.clone())
        .with_storage(stored[0].path())
        .await
        .expect("resume a ledger 8 snapshot on ledger 9");
    let fresh = Wallet::sync(&provider, seed.clone(), network.clone())
        .await
        .expect("sync from genesis across the fork");
    for (wallet, how) in [(&resumed, "resumed"), (&fresh, "synced from genesis")] {
        assert_eq!(wallet.ledger_version(), LedgerVersion::V9, "{how}");
        assert_eq!(
            wallet.balance().dust.unregistered_night_utxos,
            unregistered,
            "{how}: a UTXO registered before the fork generates no Dust after it"
        );
    }
    let submitted = provider
        .register_all_night(Duration::from_secs(420))
        .await
        .expect("register every NIGHT UTXO on ledger 9");
    assert_eq!(
        submitted, unregistered,
        "one registration for each NIGHT UTXO that the fork reset"
    );

    // What the wallet held on ledger 8 still spends on ledger 9.
    contract
        .circuits()
        .increment()
        .await
        .expect("call a ledger 8 contract on ledger 9");
    let round = contract.ledger().await.unwrap().round().unwrap();
    assert_eq!(round, 2, "the contract keeps its ledger 8 state");
    let own_unshielded = midnight_wallet::address::derive_unshielded(&seed, network.clone());
    provider
        .transfer_unshielded(NIGHT, 1_000_000, &own_unshielded)
        .await
        .expect("submit an unshielded transfer on ledger 9")
        .wait_finalized()
        .await
        .expect("an unshielded transfer on ledger 9");
    let before = provider.spendable_shielded_coins().await.unwrap();
    let token_type = before
        .first()
        .expect("a shielded coin from before the fork")
        .token_type;
    let own_shielded = midnight_wallet::address::derive_shielded(&seed, network.clone());
    provider
        .transfer_shielded(token_type, 1, &own_shielded)
        .await
        .expect("submit a shielded transfer on ledger 9")
        .wait_finalized()
        .await
        .expect("a shielded transfer on ledger 9");
    // The indexer serves blocks in order, so a wallet that syncs after it
    // served the shielded transfer also sees the unshielded one.
    let attached = received_since(&provider, &nullifiers(before)).await;

    // The other ledger 8 snapshot resumes across the fork and what came
    // after it, and a sync from genesis crosses too. Both hold what the
    // attached wallet holds.
    let resumed = Wallet::sync(&provider, seed.clone(), network.clone())
        .with_storage(stored[1].path())
        .await
        .expect("resume a ledger 8 snapshot on ledger 9");
    let fresh = Wallet::sync(&provider, seed.clone(), network)
        .await
        .expect("sync from genesis across the fork");
    for (wallet, how) in [(&resumed, "resumed"), (&fresh, "synced from genesis")] {
        assert_eq!(wallet.ledger_version(), LedgerVersion::V9, "{how}");
        assert_eq!(
            nullifiers(wallet.spendable_shielded_coins()),
            attached,
            "{how}"
        );
    }
    let generating_fresh = generating(fresh.unshielded_utxos());
    assert!(
        !generating_fresh.is_empty(),
        "the registrations on ledger 9 generate Dust"
    );
    assert_eq!(generating(resumed.unshielded_utxos()), generating_fresh);
}

/// The UTXOs that generate Dust, as `(intent hash, output index)`.
fn generating(utxos: &[midnight_provider::TrackedUtxo]) -> Vec<(Option<String>, Option<i64>)> {
    let mut generating: Vec<_> = utxos
        .iter()
        .filter(|u| u.is_registered_for_dust())
        .map(|u| (u.intent_hash.clone(), u.output_index))
        .collect();
    generating.sort();
    generating
}

async fn wait_for_ledger_9(provider: &MidnightProvider) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while provider.ledger_version().await.expect("the chain's ledger") != LedgerVersion::V9 {
        assert!(
            Instant::now() < deadline,
            "the indexer did not serve a ledger 9 block"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The attached wallet's spendable coins once the indexer has served a
/// shielded transfer to it: a coin it did not hold `before`.
async fn received_since(provider: &MidnightProvider, before: &[Nullifier]) -> Vec<Nullifier> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        provider.resync_wallet().await.expect("resync");
        let coins = nullifiers(provider.spendable_shielded_coins().await.unwrap());
        if coins.iter().any(|n| !before.contains(n)) {
            return coins;
        }
        assert!(
            Instant::now() < deadline,
            "the indexer did not serve the shielded transfer"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn nullifiers(coins: Vec<midnight_provider::SpendableShieldedCoin>) -> Vec<Nullifier> {
    let mut nullifiers: Vec<Nullifier> = coins.into_iter().map(|c| c.nullifier).collect();
    nullifiers.sort();
    nullifiers
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
