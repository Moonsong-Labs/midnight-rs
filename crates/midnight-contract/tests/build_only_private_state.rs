//! A build-only call whose witnesses change the private state refuses before
//! the build. It reserves no Dust and writes no private-state snapshot.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod secret_counter {
    compact_bindgen::contract!(
        "../../devnet/contracts/secret-counter/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::sync::Arc;
use std::time::Duration;

use midnight_contract::ContractError;
use midnight_provider::{
    DustlessBuilder, FsPrivateStateProvider, MidnightProvider, PrivateStateProvider,
};
use midnight_wallet::LocalWallet;

/// The pause between two passes of the background sync.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long the wallet waits to see the Dust spend of the deploy.
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// Each call adds one to the private state and returns the sum.
struct NextSecret;

impl secret_counter::Witnesses for NextSecret {
    type PrivateState = u64;

    fn next_secret(&self, ps: &mut u64) -> Result<u16, secret_counter::WitnessError> {
        *ps += 1;
        Ok(u16::try_from(*ps)?)
    }
}

/// A refusal after the build leaves the build's Dust reserved until its
/// reservation expires, and a refusal that writes the store leaves a snapshot of a call
/// that never reaches the chain.
#[tokio::test]
async fn a_build_only_call_that_changes_the_private_state_refuses_before_the_build() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node_url);
    let wallet = Arc::new(LocalWallet::new(
        common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await,
    ));
    let dir = tempfile::tempdir().expect("temp dir");
    let store: Arc<dyn PrivateStateProvider> = Arc::new(FsPrivateStateProvider::new(dir.path()));
    let provider = Arc::new(
        provider
            .with_wallet(Arc::clone(&wallet))
            .with_private_state(Arc::clone(&store)),
    );

    // The wallet reads finalized state, so this deploy stays invisible to
    // any other wallet on this seed until it finalizes. The next test in the
    // suite is another process on the same seed, so the test waits for it.
    let (_, deployed) = secret_counter::Contract::deploy(&*provider)
        .with_initial_state(secret_counter::LedgerInitialState::default())
        .with_zk_config(secret_counter::ZK_CONFIG_DIR)
        .send()
        .await
        .expect("submit deploy")
        .wait_finalized()
        .await
        .expect("deploy finalized");
    let contract = deployed.into_contract().await.expect("deploy");

    // The deploy's fee keeps its Dust reserved until a sync sees the spend.
    // The sync stops before the builds, so no pass changes the wallet between
    // a build and the balance read after it.
    let sync = wallet.sync_in_background(provider.clone(), SYNC_PAUSE);
    tokio::time::timeout(
        SYNC_TIMEOUT,
        sync.wait_until(|b| b.dust.spendable_speck == b.dust.balance_speck),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "no pass saw the deploy's Dust spend, last error: {:?}",
            sync.last_error()
        )
    })
    .expect("background sync");
    drop(sync);

    let mut circuits = contract.circuits().with_witnesses(&NextSecret);
    let built = circuits.contribute().build().await.map(drop);
    assert_refused(&provider, &*store, contract.address(), "build", built).await;
    let built = circuits.contribute().without_dust().await.map(drop);
    assert_refused(
        &provider,
        &*store,
        contract.address(),
        "without_dust",
        built,
    )
    .await;
}

/// Assert that the build-only call `builder` on the contract at `address`
/// gave [`ContractError::UnrecordedPrivateState`], left no Dust of the wallet
/// reserved, and wrote no snapshot to `store`.
async fn assert_refused(
    provider: &MidnightProvider,
    store: &dyn PrivateStateProvider,
    address: &str,
    builder: &str,
    built: Result<(), ContractError>,
) {
    match built {
        Err(ContractError::UnrecordedPrivateState { address: refused }) => {
            assert_eq!(refused, address, "{builder}");
        }
        other => panic!("{builder}: expected UnrecordedPrivateState, got {other:?}"),
    }
    let dust = provider.balance().await.expect("balance").dust;
    assert_eq!(
        dust.spendable_speck, dust.balance_speck,
        "{builder}: the refused call must reserve no Dust"
    );
    assert!(
        store
            .snapshots(address)
            .await
            .expect("snapshots")
            .is_empty(),
        "{builder}: the refused call must write no snapshot"
    );
}
