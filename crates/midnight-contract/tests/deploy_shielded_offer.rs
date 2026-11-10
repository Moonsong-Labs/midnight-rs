//! A deploy that carries a shielded offer applies on chain.
//!
//! The deploy proves with no fee, and the wallet then merges a part of its
//! own that spends the offer's coin, returns the change and pays the fee. Only
//! the node can say that it accepts a transaction of that shape.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! test submits from the dev seed, so it has a binary of its own. Two tests
//! that submit from one seed in one binary can select the same Dust.

mod common;

use compact_bindgen::{AlignedValue, StateValue};
use midnight_contract::{ShieldedOffer, ShieldedPayment, ShieldedTokenType};
use midnight_helpers::midnight_serialize::tagged_deserialize;
use midnight_helpers::{DefaultDB, FinalizedTransaction, Transaction};
use midnight_provider::{MidnightProvider, TransactionRequest};

/// The offer's coin and payment must reach the guaranteed Zswap offer of the
/// proven transaction, with a change output when the coin holds more than the
/// payment. A successful build keeps the coin and the fee Dust reserved, so no
/// later build of the wallet selects them. The chain must then apply the
/// deploy.
///
/// The offer pays 1 unit of shielded token id `[0; 32]` from one of the dev
/// wallet's coins back to the wallet's own keys. It is unrelated to the
/// contract being deployed (no effects-check interaction). The initial state
/// is bboard-shaped, and the deploy path does not interpret it.
#[tokio::test]
async fn deploy_funded_with_shielded_offer() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let state = common::contract_state(vec![
        StateValue::from(0u64),
        StateValue::Null,
        StateValue::from(1u64),
        StateValue::from(AlignedValue::from([0u8; 32])),
    ]);

    let provider = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&provider, &indexer_url, common::DEV_WALLET_SEED).await;
    let recipient = wallet.shielded_recipient();
    let provider = provider.with_wallet(wallet);

    // The dev wallet holds the dev devnet's default shielded token id
    // ([0; 32]) from genesis.
    let token_type = ShieldedTokenType(midnight_types::HashOutput([0u8; 32]));
    let coin = provider
        .wallet()
        .spendable_shielded_coins()
        .await
        .into_iter()
        .find(|coin| coin.token_type == token_type)
        .expect("the dev wallet must hold a coin of the default shielded token");
    let nullifier = coin.nullifier;
    let payment = ShieldedPayment {
        recipient,
        token_type,
        value: 1,
    };
    let expected_outputs = if coin.value > payment.value { 2 } else { 1 };
    let offer = ShieldedOffer {
        coins: vec![coin],
        outputs: vec![payment],
    };

    let result = midnight_contract::deploy::deploy_funded(&state, &provider, Some(offer))
        .await
        .unwrap();

    let address = result.address_hex();
    let bytes = result.transaction.into_bytes();

    // The result's reservation holds the offer's coin and the fee Dust, and
    // its bytes keep that reservation.
    let spendable = provider.wallet().spendable_shielded_coins().await;
    assert!(
        !spendable.iter().any(|c| c.nullifier == nullifier),
        "a successful deploy keeps its offer's coins reserved"
    );
    let dust = provider.wallet().balance().await.dust;
    assert!(
        dust.spendable_speck < dust.balance_speck,
        "a successful deploy keeps its fee Dust reserved"
    );

    let tx: FinalizedTransaction<DefaultDB> =
        tagged_deserialize(&mut &bytes[..]).expect("deserialize the proven deploy");
    let Transaction::Standard(stx) = &tx else {
        panic!("a deploy must be a standard transaction");
    };
    let guaranteed = stx
        .guaranteed_coins
        .as_ref()
        .expect("the deploy must carry the attached shielded offer");
    assert_eq!(
        guaranteed.inputs.len(),
        1,
        "the guaranteed offer must carry the attached coin"
    );
    assert_eq!(
        guaranteed.outputs.len(),
        expected_outputs,
        "the guaranteed offer must carry the payment, and the change when the coin holds more"
    );

    // The deploy pays its own fee, so a sponsor of its bytes adds nothing.
    let receipt = provider
        .send_transaction(TransactionRequest::sponsor(bytes.clone()))
        .await
        .expect("submit the deploy")
        .get_receipt()
        .await
        .expect("the chain must apply a deploy that carries a shielded offer");
    eprintln!(
        "deploy_funded with shielded offer: addr={address} bytes={} block={}",
        bytes.len(),
        hex::encode(receipt.block_hash),
    );
}
