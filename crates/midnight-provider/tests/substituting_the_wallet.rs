//! The provider serves a wallet it did not write, and hands back what a
//! signed transaction reserved when its submit never reaches the node.
//!
//! The provider holds any `W: MidnightWallet` and calls it only through that
//! trait. This file implements one that is not a `Wallet` and attaches it.
//! Narrowing `with_wallet` back to a concrete type, or adding a trait method
//! that only `Wallet` can satisfy, breaks this file and nothing else.
//!
//! No devnet: nothing here reaches the network.

use std::sync::{Arc, Mutex};

use midnight_helpers::{
    DefaultDB, HashMapStorage, Intent, SeedableRng, StdRng, Timestamp, Transaction, UnprovenIntent,
    UnprovenTransaction,
};
use midnight_provider::{
    HashOutput, MidnightProvider, MidnightWallet, Nullifier, ProviderError, ShieldedRecipient,
    TxEnvelope, WalletError,
};
use midnight_types::build::{self, Funded, Reservation, ReservedInput, SpentInputs};
use midnight_types::chain_pin::ChainView;
use midnight_types::{FilledTx, FundedTx, ProvenTx};

/// A wallet that holds no keys and no coins. This file never asks it to fund
/// or sign.
#[derive(Debug)]
struct StubWallet;

#[async_trait::async_trait]
impl MidnightWallet for StubWallet {
    fn shielded_recipient(&self) -> ShieldedRecipient {
        unimplemented!("this wallet holds no keys")
    }

    async fn fund_transaction(
        &self,
        _chain: &dyn ChainView,
        _tx: FilledTx,
    ) -> Result<FundedTx, WalletError> {
        Err(WalletError::Transfer("this wallet funds nothing".into()))
    }

    async fn sign_transaction(&self, _tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
        Err(WalletError::Transfer("this wallet signs nothing".into()))
    }
}

/// A signed transaction that reserved the shielded coin `nullifier`, and
/// the record of what its reservation hands back.
async fn signed_reserving(nullifier: Nullifier) -> (TxEnvelope, Arc<Mutex<Vec<Nullifier>>>) {
    let released = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&released);
    let spent = SpentInputs {
        shielded: vec![ReservedInput {
            input: nullifier,
            reserved_at: Timestamp::from_secs(100),
        }],
        ..SpentInputs::default()
    };
    let reservation = Reservation::new(spent, move |spent| {
        record
            .lock()
            .unwrap()
            .extend(spent.shielded.iter().map(|reserved| reserved.input));
    });

    let intent: UnprovenIntent<DefaultDB> =
        Intent::empty(&mut StdRng::seed_from_u64(7), Timestamp::from_secs(1));
    let tx: UnprovenTransaction<DefaultDB> =
        Transaction::from_intents("undeployed", HashMapStorage::new().insert(1, intent));
    let sealed = tx.mock_prove().expect("mock prove");

    let proven = build::prove(build::funded(
        Funded::Unchanged(Box::new(sealed)),
        reservation,
    ))
    .await
    .expect("nothing to prove");
    (build::envelope(proven).expect("serialize"), released)
}

/// A prepare sends nothing to the node, so a signed transaction whose prepare
/// fails gets its inputs back, whatever the error. A failed connect is
/// `ProviderError::Rpc`, so a release that follows the error variant keeps
/// the inputs reserved until their TTL.
#[tokio::test]
async fn a_reserved_transaction_whose_connect_fails_gets_its_inputs_back() {
    // A URL that does not parse fails the connect at once, with no retry.
    let provider = MidnightProvider::new("not a node url").with_wallet(StubWallet);
    let nullifier = Nullifier(HashOutput([7; 32]));
    let (envelope, released) = signed_reserving(nullifier).await;

    let Err(err) = provider.submit_reserved(envelope).await else {
        panic!("a submit whose connect fails must fail");
    };

    assert!(matches!(err, ProviderError::Rpc(_)), "{err:?}");
    assert_eq!(*released.lock().unwrap(), vec![nullifier]);
}
