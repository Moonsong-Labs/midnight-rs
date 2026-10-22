//! A deploy that carries a shielded offer applies on chain.
//!
//! The deploy proves with no fee, and the wallet then merges a fee intent of
//! its own. Only the node can say that it accepts a transaction of that shape.
//!
//! Gated on a running devnet (`MIDNIGHT_NODE_URL`, `MIDNIGHT_INDEXER_URL`).
//! Under `make test-e2e`, which sets `MIDNIGHT_E2E`, a missing URL panics. The
//! test submits from the dev seed, so it has a binary of its own. Two tests
//! that submit from one seed in one binary can select the same Dust.

mod common;

use compact_bindgen::{AlignedValue, StateValue};
use midnight_contract::{ShieldedOffer, ShieldedPayment, ShieldedTokenType};
use midnight_provider::MidnightProvider;

/// The offer's coin and payment must reach the guaranteed Zswap offer of the
/// proven transaction, with a change output when the coin holds more than the
/// payment. A successful build keeps the coin reserved, so no later build of
/// the wallet selects it, and its result names the coin and the fee Dust for
/// the submit to hand back. The chain must then apply the deploy.
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

    let provider = MidnightProvider::new(&node_url, &indexer_url);
    let wallet = common::synced_wallet(&provider, common::DEV_WALLET_SEED).await;
    let recipient = wallet.seed().shielded_recipient();
    let provider = provider.with_wallet(wallet);

    // The dev wallet holds the dev devnet's default shielded token id
    // ([0; 32]) from genesis.
    let token_type = ShieldedTokenType(midnight_types::HashOutput([0u8; 32]));
    let coin = provider
        .spendable_shielded_coins()
        .await
        .unwrap()
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

    // The guaranteed offer's input and output counts, read with the types of
    // the transaction's generation.
    macro_rules! guaranteed_counts {
        ($ledger:ident) => {{
            use midnight_helpers::$ledger as l;
            let tx: l::FinalizedTransaction<midnight_helpers::DefaultDB> =
                midnight_helpers::midnight_serialize::tagged_deserialize(
                    &mut &result.transaction.tx_bytes[..],
                )
                .expect("deserialize the proven deploy");
            let l::Transaction::Standard(stx) = &tx else {
                panic!("a deploy must be a standard transaction");
            };
            let guaranteed = stx
                .guaranteed_coins
                .as_ref()
                .expect("the deploy must carry the attached shielded offer");
            (guaranteed.inputs.len(), guaranteed.outputs.len())
        }};
    }
    let (inputs, outputs) = match midnight_types::LedgerVersion::of_transaction(
        &result.transaction.tx_bytes,
    )
    .unwrap()
    {
        midnight_types::LedgerVersion::V8 => guaranteed_counts!(ledger_8),
        midnight_types::LedgerVersion::V9 => guaranteed_counts!(ledger_9),
    };
    assert_eq!(
        inputs, 1,
        "the guaranteed offer must carry the attached coin"
    );
    assert_eq!(
        outputs, expected_outputs,
        "the guaranteed offer must carry the payment, and the change when the coin holds more"
    );

    let spendable = provider.spendable_shielded_coins().await.unwrap();
    assert!(
        !spendable.iter().any(|c| c.nullifier == nullifier),
        "a successful deploy keeps its offer's coins reserved"
    );

    // The submit hands back only what the result names, so it must name the
    // offer's coin and the fee Dust, which two build contexts reserved.
    let spent = &result.transaction.spent;
    assert!(
        spent.shielded.iter().any(|coin| coin.input == nullifier),
        "the deploy result must name the offer's coin: {spent:?}"
    );
    assert!(
        !spent.dust.is_empty(),
        "the deploy result must name the fee Dust: {spent:?}"
    );

    let address = result.address_hex();
    let bytes = result.transaction.tx_bytes.len();
    let (in_block, _) = provider
        .submit_reserved(result.transaction)
        .await
        .expect("submit the deploy")
        .wait_finalized()
        .await
        .expect("the chain must apply a deploy that carries a shielded offer");
    eprintln!(
        "deploy_funded with shielded offer: addr={address} bytes={bytes} block={}",
        hex::encode(in_block.block_hash),
    );
}
