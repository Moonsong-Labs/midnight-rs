//! A deploy runs the contract's constructor with typed arguments and
//! witnesses, and records the private state that the constructor leaves.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics.

mod common;

mod owned_counter {
    compact_bindgen::contract!(
        "../../devnet/contracts/owned-counter/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::sync::Arc;
use std::time::Duration;

use compact_bindgen::{AlignedValue, Bytes};
use midnight_provider::{
    FsPrivateStateProvider, MidnightProvider, PrivateStateProvider, SnapshotStatus,
};
use midnight_wallet::LocalWallet;

/// The pause between two passes of the background sync.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long the wallet waits to see the Dust spend of the deploy.
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// The owner's secret, and the count of the witness calls.
type PrivateState = ([u8; 32], u32);

const SECRET: [u8; 32] = [7; 32];

/// Returns the secret of the private state, and counts its calls there.
struct OwnerKey;

impl owned_counter::Witnesses for OwnerKey {
    type PrivateState = PrivateState;

    fn owner_secret(
        &self,
        ps: &mut PrivateState,
    ) -> Result<Bytes<32>, owned_counter::WitnessError> {
        ps.1 += 1;
        Ok(Bytes(ps.0))
    }
}

/// `owner_key(sk)` of the contract: the persistent hash of the padded domain
/// and `sk`.
fn owner_key(sk: [u8; 32]) -> AlignedValue {
    let domain = b"owned-counter:pk:";
    let mut pad = [0u8; 32];
    pad[..domain.len()].copy_from_slice(domain);
    common::persistent_hash(AlignedValue::concat(
        [AlignedValue::from(pad), AlignedValue::from(sk)].iter(),
    ))
}

/// A deploy that drops the constructor's arguments, its witnesses or the
/// initial private state deploys another round or another owner. A deploy
/// that does not record the private state makes the first call start from
/// `Default`, whose key is not the owner's, so the owner's `bump` fails.
#[tokio::test]
async fn a_deploy_runs_the_constructor_and_records_its_private_state() {
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

    // Only a finality wait confirms the snapshot of the deploy.
    let (_, pending) = owned_counter::Contract::deploy(&*provider)
        .with_witnesses(&OwnerKey)
        .with_initial_private_state((SECRET, 0))
        .with_constructor_args(5)
        .with_zk_config(owned_counter::ZK_CONFIG_DIR)
        .send()
        .await
        .expect("deploy")
        .wait_finalized()
        .await
        .expect("the deploy finalizes");
    let contract = pending
        .into_contract()
        .await
        .expect("the deployed contract");

    let ledger = contract.ledger().await.expect("ledger");
    assert_eq!(
        ledger.round().expect("round"),
        5,
        "the constructor starts the round at its argument"
    );
    assert_eq!(
        AlignedValue::from(ledger.owner().expect("owner")),
        owner_key(SECRET),
        "the constructor's witness reads the initial private state"
    );

    let snapshots = store
        .snapshots(contract.address())
        .await
        .expect("snapshots");
    let [snapshot] = snapshots.as_slice() else {
        panic!("expected one snapshot of the deploy, got {snapshots:?}");
    };
    assert_eq!(snapshot.status, SnapshotStatus::Confirmed);
    let recorded: PrivateState =
        serde_json::from_slice(&snapshot.data).expect("the snapshot decodes");
    assert_eq!(
        recorded,
        (SECRET, 1),
        "the deploy records the private state after the constructor's witness call"
    );

    // The deploy's fee keeps its Dust reserved until a sync sees the spend.
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

    // A call finalizes, so the next test on this seed sees the deploy too.
    contract
        .circuits()
        .with_witnesses(&OwnerKey)
        .bump()
        .await
        .expect("the owner bumps");
    assert_eq!(
        contract
            .ledger()
            .await
            .expect("ledger")
            .round()
            .expect("round"),
        6
    );
}
