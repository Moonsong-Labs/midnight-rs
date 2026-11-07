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

use midnight_helpers::ledger_9::{
    BuildContext, FinalizedTransaction, ProofProvider, StandardTransactionInfo,
};
use midnight_helpers::{DefaultDB, StdRng};
use midnight_provider::{
    ChainParameters, CoinInfo, HashOutput, MidnightProvider, Network, Nullifier, ProviderError,
    ShieldedRecipient, ShieldedTokenType, SpendableShieldedCoin, SpentInputs, SyncCursors,
    TrackedUtxo, TransferRequest, TransferResult, WalletBalance, WalletError, WalletFacade,
};
use midnight_types::Timestamp;
use midnight_types::ledger_9::PreparedInput;
use midnight_wallet::chain_pin::ChainView;
use midnight_wallet_facade::ledger_9::{ReservedBuild, WalletBuilds};

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

    async fn shielded_recipient(&self) -> ShieldedRecipient {
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

    async fn persist_reservation(&self, _spent: &SpentInputs) {}

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

/// The stub's builds: it refuses every one.
#[async_trait::async_trait]
impl WalletBuilds for StubWallet {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        Err(WalletError::Sync("stub wallet has no chain state".into()))
    }

    async fn add_funding(&self, _context: &BuildContext) -> Result<(), WalletError> {
        Err(WalletError::Sync("stub wallet funds nothing".into()))
    }

    async fn prepare_transfer(
        &self,
        _request: TransferRequest,
        _proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Result<ReservedBuild, WalletError> {
        Err(WalletError::Transfer("stub wallet builds nothing".into()))
    }

    async fn prepare_funded(
        &self,
        _tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
    ) -> Result<ReservedBuild, WalletError> {
        Err(WalletError::Transfer("stub wallet funds nothing".into()))
    }

    async fn spend_shielded(
        &self,
        _context: &Arc<BuildContext>,
        _nullifiers: Vec<Nullifier>,
        _rng: &mut StdRng,
    ) -> Result<(Vec<PreparedInput>, SpentInputs), WalletError> {
        unimplemented!("this wallet holds no coins")
    }

    async fn prepare_fees(
        &self,
        _tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        _external: &FinalizedTransaction<DefaultDB>,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        Err(WalletError::Transfer("stub wallet funds nothing".into()))
    }
}

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
    use midnight_helpers::ledger_9::SeedableRng;

    let provider = MidnightProvider::new("ws://test").with_wallet(StubWallet::default());
    let builds = provider.builds().await.expect("the stub resyncs");
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
            &mut StdRng::seed_from_u64(0),
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
/// the inputs reserved until their TTL.
#[tokio::test]
async fn a_reserved_build_whose_connect_fails_gets_its_inputs_back() {
    let wallet = Arc::new(StubWallet::default());
    // A URL that does not parse fails the connect at once, with no retry.
    let provider = MidnightProvider::new("not a node url").with_wallet(wallet.clone());
    let nullifier = Nullifier(HashOutput([7; 32]));
    let result = TransferResult {
        tx_bytes: vec![0; 8],
        spent: SpentInputs::from_shielded(vec![nullifier], Timestamp::from_secs(100)),
        fee_speck: 0,
    };

    let Err(err) = provider.submit_reserved(result).await else {
        panic!("a submit whose connect fails must fail");
    };

    assert!(matches!(err, ProviderError::Rpc(_)), "{err:?}");
    assert_eq!(*wallet.released.lock().unwrap(), vec![nullifier]);
}
