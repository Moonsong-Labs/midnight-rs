//! The provider serves a wallet it did not write.
//!
//! It holds `Arc<dyn WalletFacade>`, so a reading it answers comes from
//! whatever implements that trait. This file implements one that is not a
//! `Wallet` and attaches it.
//!
//! The test is mostly its own compilation. Narrowing `with_wallet` back to a
//! concrete type, or adding a trait method only `Wallet` can satisfy (one
//! handing out a `&Wallet`, or a lock guard), breaks this file and nothing
//! else.
//!
//! The stub also records what the provider hands back to it. A submit that
//! never reaches the node must release each reservation of the build before
//! it returns.
//!
//! No devnet: nothing here reaches the network.

use std::sync::{Arc, Mutex};

use midnight_provider::{
    ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, HashOutput, LedgerVersion,
    MidnightProvider, Network, Nullifier, ProviderError, SpendableShieldedCoin, SpentInputs,
    SubmitError, SyncCursors, TrackedUtxo, TransferRequest, WalletBalance, WalletError,
    WalletFacade, WalletSeed,
};
use midnight_types::Timestamp;
use midnight_wallet::chain_pin::ChainView;

/// A wallet that answers three readings, records each release by its
/// `reserved_at`, and refuses the rest. Everything it refuses would need
/// chain state to answer honestly.
#[derive(Default)]
struct StubWallet {
    released: Arc<Mutex<Vec<Timestamp>>>,
}

#[async_trait::async_trait]
impl WalletFacade for StubWallet {
    async fn network(&self) -> Network {
        Network::Preprod
    }

    async fn ledger_version(&self) -> LedgerVersion {
        LedgerVersion::V9
    }

    async fn sync_cursors(&self) -> SyncCursors {
        SyncCursors {
            last_block_height: 12,
            last_tx_id: Some(34),
            zswap_event_id: 56,
            dust_event_id: 78,
        }
    }

    async fn dust_synced(&self) -> bool {
        true
    }

    async fn seed(&self) -> WalletSeed {
        unimplemented!("this wallet holds no seed")
    }

    async fn shielded_public_keys(&self) -> (CoinPublicKey, EncryptionPublicKey) {
        unimplemented!("this wallet holds no keys")
    }

    async fn balance(&self) -> WalletBalance {
        unimplemented!("this wallet holds no coins")
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        unimplemented!("this wallet holds no coins")
    }

    async fn unshielded_utxos(&self) -> Vec<TrackedUtxo> {
        Vec::new()
    }

    async fn parameters(&self) -> ChainParameters {
        unimplemented!("this wallet has synced no parameters")
    }

    async fn release(&self, spent: &SpentInputs) {
        self.released.lock().unwrap().push(spent.reserved_at);
    }

    async fn resync(&self, _chain: &dyn ChainView) -> Result<(), WalletError> {
        Ok(())
    }

    async fn rescan_shielded(&self) -> Result<(), WalletError> {
        Ok(())
    }

    async fn watch_for_coins(&self, _coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        Ok(())
    }

    async fn forget_coins(&self, _coins: Vec<CoinInfo>) -> Result<(), WalletError> {
        Ok(())
    }
}

/// The stub's builds, the same on each generation: it refuses every one.
macro_rules! stub_builds {
    ($ledger:ident) => {
        #[async_trait::async_trait]
        impl midnight_wallet_facade::$ledger::WalletBuilds for StubWallet {
            async fn execution_context(
                &self,
            ) -> Result<Arc<midnight_helpers::$ledger::BuildContext>, WalletError> {
                Err(WalletError::Sync("stub wallet has no chain state".into()))
            }

            async fn add_funding(
                &self,
                _context: &midnight_helpers::$ledger::BuildContext,
            ) -> Result<(), WalletError> {
                Err(WalletError::Sync("stub wallet funds nothing".into()))
            }

            async fn prepare_transfer(
                &self,
                _request: TransferRequest,
                _proof_provider: Arc<
                    dyn midnight_helpers::$ledger::ProofProvider<midnight_helpers::DefaultDB>,
                >,
            ) -> Result<midnight_wallet_facade::$ledger::ReservedBuild, WalletError> {
                Err(WalletError::Transfer("stub wallet builds nothing".into()))
            }

            async fn prepare_funded(
                &self,
                _tx_info: midnight_helpers::$ledger::StandardTransactionInfo<
                    midnight_helpers::DefaultDB,
                    midnight_helpers::$ledger::BuildContext,
                >,
            ) -> Result<midnight_wallet_facade::$ledger::ReservedBuild, WalletError> {
                Err(WalletError::Transfer("stub wallet funds nothing".into()))
            }

            async fn spend_shielded(
                &self,
                _context: &Arc<midnight_helpers::$ledger::BuildContext>,
                _nullifiers: Vec<Nullifier>,
                _rng: &mut midnight_helpers::StdRng,
            ) -> Result<(Vec<midnight_types::$ledger::PreparedInput>, SpentInputs), WalletError>
            {
                unimplemented!("this wallet holds no coins")
            }

            async fn prepare_fees(
                &self,
                _tx_info: midnight_helpers::$ledger::StandardTransactionInfo<
                    midnight_helpers::DefaultDB,
                    midnight_helpers::$ledger::BuildContext,
                >,
                _external: &midnight_helpers::$ledger::FinalizedTransaction<
                    midnight_helpers::DefaultDB,
                >,
            ) -> Result<Option<midnight_wallet_facade::$ledger::ReservedBuild>, WalletError> {
                Err(WalletError::Transfer("stub wallet funds nothing".into()))
            }
        }
    };
}

stub_builds!(ledger_8);
stub_builds!(ledger_9);

#[tokio::test]
async fn the_provider_reads_whatever_wallet_it_was_given() {
    let provider = MidnightProvider::new("ws://test", "http://test")
        .expect("provider")
        .with_wallet(StubWallet::default());

    assert_eq!(
        provider.network().await.expect("attached"),
        Network::Preprod
    );
    assert!(provider.dust_synced().await.expect("attached"));
    assert_eq!(
        provider
            .sync_cursors()
            .await
            .expect("attached")
            .zswap_event_id,
        56
    );
}

/// A release matches on `reserved_at`, so one merged entry would hand back
/// nothing for the other half. A release left to the guards' `Drop` runs in a
/// spawned task, after the caller already reads the inputs as reserved.
#[tokio::test]
async fn a_submit_that_never_reached_the_node_hands_back_each_reservation() {
    let wallet = StubWallet::default();
    let released = wallet.released.clone();
    // A node URL that does not parse fails the prepare before any dial.
    let provider = MidnightProvider::new("not a node url", "http://test")
        .expect("provider")
        .with_wallet(wallet);
    let pinned = SpentInputs::from_shielded(
        vec![Nullifier(HashOutput([1; 32]))],
        Timestamp::from_secs(1),
    );
    let fee = SpentInputs::from_shielded(
        vec![Nullifier(HashOutput([2; 32]))],
        Timestamp::from_secs(2),
    );
    let expected = vec![pinned.reserved_at, fee.reserved_at];

    let Err(err) = provider.submit_reserved(&[0; 8], vec![pinned, fee]).await else {
        panic!("there is no node to submit to");
    };

    assert!(
        matches!(
            err,
            ProviderError::Submission(SubmitError::NotSubmitted { .. })
        ),
        "a submit that never reached the node must be NotSubmitted, got {err:?}"
    );
    // Read with no await since the submit returned: on this single-threaded
    // runtime, a task that `Drop` spawned has not run yet.
    assert_eq!(*released.lock().unwrap(), expected);
}
