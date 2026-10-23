//! A deploy or a call that the node never gets hands its inputs back to the
//! wallet.
//!
//! The deploy builds against a synced wallet, and its submit dials a node that
//! refuses the connection. The call builds against the devnet, and its
//! private-state store refuses the `Pending` snapshot, so the call stops after
//! its prepare and before its submit. Neither transaction left the process, so
//! the wallet must get the fee Dust back at once. A deploy that submits with
//! plain `submit`, or a call that prepares with plain `prepare`, keeps that
//! Dust reserved until its TTL passes, and the next fee-paying build then
//! fails with `InsufficientDust`.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! test builds from the dev seed, so it has a binary of its own, and both
//! cases run in one test function: two tests that submit from one seed in one
//! binary can race.

mod common;

mod counter {
    compact_bindgen::contract!("../../devnet/contracts/counter/compiled/compiler/analyzed-ir.sexp");
}

mod secret_counter {
    compact_bindgen::contract!(
        "../../devnet/contracts/secret-counter/compiled/compiler/analyzed-ir.sexp"
    );
}

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use midnight_contract::ContractError;
use midnight_provider::{
    EncryptedExport, ExportOptions, FsPrivateStateProvider, ImportOptions, ImportResult,
    MidnightProvider, PrivateStateError, PrivateStateProvider, ProviderError, Snapshot,
    WalletFacade,
};
use midnight_wallet::LocalWallet;

/// A node URL whose dial fails: nothing listens on port 1.
const REFUSING_NODE_URL: &str = "ws://127.0.0.1:1";

/// The pause between two passes of the background sync.
const SYNC_PAUSE: Duration = Duration::from_secs(1);

/// How long the wallet waits to see the Dust spend of the deploy.
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// Each call adds one to the private state, so each call records a snapshot.
struct NextSecret;

impl secret_counter::Witnesses for NextSecret {
    type PrivateState = u64;

    fn next_secret(&self, ps: &mut u64) -> Result<u16, secret_counter::WitnessError> {
        *ps += 1;
        Ok(u16::try_from(*ps)?)
    }
}

/// A private-state store that refuses every `append_pending`, and forwards
/// every other method to a store on disk.
struct RefusesSnapshots(FsPrivateStateProvider);

#[async_trait]
impl PrivateStateProvider for RefusesSnapshots {
    async fn append_pending(
        &self,
        _address: &str,
        _extrinsic_hash: [u8; 32],
        _depends_on: Option<[u8; 32]>,
        _state: &[u8],
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Io(
            "this store refuses every snapshot".into(),
        ))
    }

    async fn confirm(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
        block_height: Option<u64>,
        block_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        self.0
            .confirm(address, extrinsic_hash, block_height, block_hash)
            .await
    }

    async fn mark_failed(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        self.0.mark_failed(address, extrinsic_hash).await
    }

    async fn head_with_extrinsic(
        &self,
        address: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 32])>, PrivateStateError> {
        self.0.head_with_extrinsic(address).await
    }

    async fn snapshots(&self, address: &str) -> Result<Vec<Snapshot>, PrivateStateError> {
        self.0.snapshots(address).await
    }

    async fn rollback_from(
        &self,
        address: &str,
        extrinsic_hash: [u8; 32],
    ) -> Result<(), PrivateStateError> {
        self.0.rollback_from(address, extrinsic_hash).await
    }

    async fn forget(&self, address: &str) -> Result<(), PrivateStateError> {
        self.0.forget(address).await
    }

    async fn forget_all(&self) -> Result<(), PrivateStateError> {
        self.0.forget_all().await
    }

    async fn set_signing_key(&self, address: &str, key: &[u8]) -> Result<(), PrivateStateError> {
        self.0.set_signing_key(address, key).await
    }

    async fn get_signing_key(&self, address: &str) -> Result<Option<Vec<u8>>, PrivateStateError> {
        self.0.get_signing_key(address).await
    }

    async fn remove_signing_key(&self, address: &str) -> Result<(), PrivateStateError> {
        self.0.remove_signing_key(address).await
    }

    async fn clear_signing_keys(&self) -> Result<(), PrivateStateError> {
        self.0.clear_signing_keys().await
    }

    async fn export_private_states(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        self.0.export_private_states(opts).await
    }

    async fn import_private_states(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        self.0.import_private_states(data, opts).await
    }

    async fn export_signing_keys(
        &self,
        opts: &ExportOptions,
    ) -> Result<EncryptedExport, PrivateStateError> {
        self.0.export_signing_keys(opts).await
    }

    async fn import_signing_keys(
        &self,
        data: &EncryptedExport,
        opts: &ImportOptions,
    ) -> Result<ImportResult, PrivateStateError> {
        self.0.import_signing_keys(data, opts).await
    }
}

#[tokio::test]
async fn a_deploy_or_a_call_that_the_node_never_gets_hands_its_inputs_back() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    // The sync reads the network and the chain pin from the devnet's node. The
    // build reads no node: its resync finds no answer for the pin, keeps the
    // state, and replays from the indexer. Only the submit needs the node.
    let devnet = MidnightProvider::new(&node_url);
    let wallet = Arc::new(LocalWallet::new(
        common::synced_wallet(&devnet, &indexer_url, common::DEV_WALLET_SEED).await,
    ));
    let refusing = MidnightProvider::new(REFUSING_NODE_URL).with_wallet(wallet.clone());

    let Err(err) = counter::Contract::deploy(&refusing)
        .with_initial_state(counter::LedgerInitialState::default())
        .with_zk_config(counter::ZK_CONFIG_DIR)
        .send()
        .await
    else {
        panic!("a deploy whose node refuses the dial must fail");
    };
    assert!(
        matches!(err, ContractError::Provider(ProviderError::Rpc(_))),
        "the deploy must fail at the connect of its submit, after a build that reserved its fee: \
         {err:?}"
    );
    assert_no_dust_reserved(
        &wallet,
        "the deploy kept its fee Dust reserved after a submit that never reached the node",
    )
    .await;

    let dir = tempfile::tempdir().expect("temp dir");
    let store: Arc<dyn PrivateStateProvider> =
        Arc::new(RefusesSnapshots(FsPrivateStateProvider::new(dir.path())));
    let provider = Arc::new(
        devnet
            .with_wallet(Arc::clone(&wallet))
            .with_private_state(store),
    );
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
    // The sync stops before the call, so no pass changes the wallet between
    // the call and the balance read after it.
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
    let called = circuits.contribute().await;
    assert!(
        matches!(called, Err(ContractError::PendingSnapshotFailed { .. })),
        "the call must stop at the snapshot, after a build that reserved its fee: {called:?}"
    );
    assert_no_dust_reserved(
        &wallet,
        "the call kept its fee Dust reserved after it stopped before the submit",
    )
    .await;
}

/// Assert that `wallet` holds no reserved Dust. `failure` says which build
/// kept it.
async fn assert_no_dust_reserved(wallet: &LocalWallet, failure: &str) {
    let dust = wallet.balance().await.dust;
    assert_eq!(dust.spendable_speck, dust.balance_speck, "{failure}");
}
