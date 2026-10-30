//! The provider serves a wallet it did not write.
//!
//! It holds `Arc<dyn WalletFacade>`, so a reading it answers comes from
//! whatever implements that trait. This file implements one that is not a
//! `Wallet` and attaches it.
//!
//! The file is mostly its own compilation. Narrowing `with_wallet` back to a
//! concrete type, or adding a trait method only `Wallet` can satisfy (one
//! handing out a `&Wallet`, or a lock guard), breaks this file and nothing
//! else.
//!
//! No devnet: nothing here reaches the network.

use std::sync::Arc;

use midnight_provider::{
    Builds, ChainParameters, CoinInfo, CoinPublicKey, EncryptionPublicKey, HashOutput,
    LedgerVersion, MidnightProvider, Network, Nullifier, ProviderError, ShieldedTokenType,
    SpendableShieldedCoin, SpentInputs, SyncCursors, TrackedUtxo, TransferRequest, TransferResult,
    WalletBalance, WalletError, WalletFacade,
};
use midnight_types::Timestamp;
use midnight_wallet::chain_pin::ChainView;

/// A wallet that answers the three readings this test makes and refuses the
/// rest. Everything it refuses would need chain state to answer honestly. It
/// records the shielded inputs that the provider hands back.
#[derive(Default)]
struct StubWallet {
    released: std::sync::Mutex<Vec<Nullifier>>,
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
        let mut released = self.released.lock().unwrap();
        released.extend(spent.shielded.iter().map(|reserved| reserved.input));
    }

    async fn resync(&self, _chain: &dyn ChainView) -> Result<(), WalletError> {
        Ok(())
    }

    async fn rescan_shielded(&self, _chain: &dyn ChainView) -> Result<(), WalletError> {
        Ok(())
    }

    async fn watch_for_coins(
        &self,
        _coins: Vec<CoinInfo>,
        _chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
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
                _ttl: Option<std::time::Duration>,
            ) -> Result<midnight_wallet_facade::$ledger::ReservedBuild, WalletError> {
                Err(WalletError::Transfer("stub wallet funds nothing".into()))
            }

            async fn spend_shielded(
                &self,
                _context: &Arc<midnight_helpers::$ledger::BuildContext>,
                _nullifiers: Vec<Nullifier>,
                _ttl: Option<std::time::Duration>,
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
    let provider = MidnightProvider::new("ws://test").with_wallet(StubWallet::default());

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

/// A wallet the SDK did not write may skip the duplicate check that
/// `WalletBuilds::spend_shielded` asks for. A second spend of one coin then
/// counts its value twice. The stub's `spend_shielded` panics, so a provider
/// that leaves the check to the wallet fails here.
#[tokio::test]
async fn the_provider_refuses_a_coin_named_twice_before_the_wallet_spends() {
    use midnight_helpers::ledger_9::{BuildContext, SeedableRng};

    let provider = MidnightProvider::new("ws://test").with_wallet(StubWallet::default());
    let Builds::Ledger9(builds) = provider.builds().await.expect("the stub resyncs") else {
        panic!("the stub's state is on ledger 9");
    };
    let coin = SpendableShieldedCoin {
        token_type: ShieldedTokenType(HashOutput([0; 32])),
        value: 1,
        nonce: [0; 32],
        nullifier: Nullifier(HashOutput([7; 32])),
    };
    let context = Arc::new(BuildContext::new_from_wallet_seeds("undeployed", &[]));

    let result = builds
        .prepare_shielded_inputs(
            &context,
            &[coin.clone(), coin],
            None,
            &mut midnight_helpers::StdRng::seed_from_u64(0),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(ProviderError::Wallet(WalletError::DuplicateInput { .. }))
        ),
        "got {:?}",
        result.err()
    );
}

/// A prepare sends nothing to the node, so a reserved build whose prepare
/// fails gets its inputs back, whatever the error. A failed connect is
/// `ProviderError::Rpc`, so a release that follows the error variant keeps
/// the inputs reserved until their reservation expires.
#[tokio::test]
async fn a_reserved_build_whose_connect_fails_gets_its_inputs_back() {
    let wallet = Arc::new(StubWallet::default());
    // A URL that does not parse fails the connect at once, with no retry.
    let provider = MidnightProvider::new("not a node url").with_wallet(wallet.clone());
    let nullifier = Nullifier(HashOutput([7; 32]));
    let result = TransferResult {
        tx_bytes: vec![0; 8],
        ledger_version: LedgerVersion::V9,
        spent: SpentInputs::from_shielded(vec![nullifier], Timestamp::from_secs(100)),
        fee_speck: 0,
    };

    let Err(err) = provider.submit_reserved(result).await else {
        panic!("a submit whose connect fails must fail");
    };

    assert!(matches!(err, ProviderError::Rpc(_)), "{err:?}");
    assert_eq!(*wallet.released.lock().unwrap(), vec![nullifier]);
}
