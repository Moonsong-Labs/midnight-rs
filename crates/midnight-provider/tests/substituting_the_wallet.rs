//! The provider serves a wallet it did not write.
//!
//! It holds `Arc<dyn WalletFacade>`, so what it asks of the wallet goes to
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

use midnight_helpers::{
    BuildContext, DefaultDB, FinalizedTransaction, ProofProvider, StandardTransactionInfo,
};
use midnight_provider::{
    CoinInfo, HashOutput, MidnightProvider, Nullifier, ProviderError, ShieldedRecipient,
};
use midnight_types::{
    Cover, ReservedInput, SpendableShieldedCoin, SpentInputs, Timestamp, TransferRequest,
    TransferResult, WalletBalance, WalletError,
};
use midnight_wallet::chain_pin::ChainView;
use midnight_wallet_facade::{ReservedBuild, WalletBuilds, WalletFacade};

/// A wallet whose resync, coin watch and reservation persist do nothing, and
/// that refuses everything that would need chain state to answer honestly.
/// It records the shielded inputs that the provider hands back.
#[derive(Default)]
struct StubWallet {
    released: std::sync::Mutex<Vec<Nullifier>>,
}

#[async_trait::async_trait]
impl WalletFacade for StubWallet {
    async fn shielded_recipient(&self) -> ShieldedRecipient {
        unimplemented!("this wallet holds no keys")
    }

    async fn balance(&self) -> WalletBalance {
        unimplemented!("this wallet holds no coins")
    }

    async fn spendable_shielded_coins(&self) -> Vec<SpendableShieldedCoin> {
        unimplemented!("this wallet holds no coins")
    }

    async fn release(&self, spent: &SpentInputs) {
        let mut released = self.released.lock().unwrap();
        released.extend(spent.shielded.iter().map(|reserved| reserved.input));
    }

    async fn persist_reservation(&self, _spent: &SpentInputs) {}

    async fn resync(&self, _chain: &dyn ChainView) -> Result<(), WalletError> {
        Ok(())
    }

    async fn watch_for_coins(
        &self,
        _coins: Vec<CoinInfo>,
        _chain: &dyn ChainView,
    ) -> Result<(), WalletError> {
        Ok(())
    }
}

/// The stub's builds: it refuses every one.
#[async_trait::async_trait]
impl WalletBuilds for StubWallet {
    async fn execution_context(&self) -> Result<Arc<BuildContext>, WalletError> {
        Err(WalletError::Sync("stub wallet has no chain state".into()))
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

    async fn prepare_fees(
        &self,
        _tx_info: StandardTransactionInfo<DefaultDB, BuildContext>,
        _external: &FinalizedTransaction<DefaultDB>,
        _cover: Cover,
    ) -> Result<Option<ReservedBuild>, WalletError> {
        Err(WalletError::Transfer("stub wallet funds nothing".into()))
    }
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
        spent: SpentInputs {
            shielded: vec![ReservedInput {
                input: nullifier,
                reserved_at: Timestamp::from_secs(100),
            }],
            ..SpentInputs::default()
        },
        fee_speck: 0,
    };

    let Err(err) = provider.submit_reserved(result).await else {
        panic!("a submit whose connect fails must fail");
    };

    assert!(matches!(err, ProviderError::Rpc(_)), "{err:?}");
    assert_eq!(*wallet.released.lock().unwrap(), vec![nullifier]);
}
