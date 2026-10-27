//! One transfer that pays several recipients.
//!
//! The list forms put every payment in one offer. The chain must accept that
//! offer, and each recipient must receive its own amount. No build-time check
//! shows that, so this test submits and reads what each recipient holds. It
//! also merges a Dustless list of each kind into one transaction, which
//! `balance_transaction` funds.
//!
//! Needs a devnet. Every build funds from the genesis wallet, so the builds
//! stay in one fn: a second fn would race it for the same Dust.

#[expect(dead_code, reason = "this test needs no synced_provider")]
mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use midnight_provider::{
    DustlessBuilder, HashOutput, LedgerVersion, MidnightProvider, NIGHT, Network, Payment, Seed,
    ShieldedTokenType, TransferResult, UnshieldedTokenType,
};
use midnight_wallet::WalletBuilder;

/// The shielded token the dev preset mints with the all-zero id. The chain
/// refuses a transfer of the other tokens it mints after inclusion, so only
/// this one is submitted.
const DEFAULT_SHIELDED_TOKEN: ShieldedTokenType = ShieldedTokenType(HashOutput([0; 32]));

/// What each of the two recipients receives, in each kind. They differ, so a
/// build that pays one recipient the amount of the other fails.
const AMOUNTS: [u128; 2] = [3, 5];

#[tokio::test]
async fn a_transfer_to_many_pays_each_recipient_its_own_amount() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let funder = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&funder, &indexer_url, common::DEV_WALLET_SEED).await;
    let own_shielded = wallet.shielded_address();
    let funder = funder.with_wallet(wallet);

    // New seeds, so what a recipient holds afterwards is what this test paid.
    let recipients = [Seed::generate(), Seed::generate()];

    let shielded = funder
        .transfer_shielded_many(shielded_payments(&recipients))
        .build()
        .await
        .expect("build the shielded transfer");
    assert_no_surplus(&shielded.tx_bytes, "the shielded transfer");
    submit(&funder, shielded, "the shielded transfer").await;

    let unshielded = funder
        .transfer_unshielded_many(unshielded_payments(&recipients))
        .build()
        .await
        .expect("build the unshielded transfer");
    assert_no_surplus(&unshielded.tx_bytes, "the unshielded transfer");
    submit(&funder, unshielded, "the unshielded transfer").await;

    assert_each_received_its_amounts(&node_url, &indexer_url, &recipients).await;

    // The chain must accept the signed unshielded intent beside a fee intent
    // that a later call added.
    let recipients = [Seed::generate(), Seed::generate()];
    let shielded = funder
        .transfer_shielded_many(shielded_payments(&recipients))
        .without_dust()
        .await
        .expect("build the Dustless shielded transfer");
    let unshielded = funder
        .transfer_unshielded_many(unshielded_payments(&recipients))
        .without_dust()
        .await
        .expect("build the Dustless unshielded transfer");
    let merged = funder
        .merge_transactions(&[shielded.into_bytes(), unshielded.into_bytes()])
        .expect("merge the Dustless transfers");
    let both_kinds = funder
        .balance_transaction(&merged)
        .await
        .expect("pay the fee of the merged transfers");
    assert_no_surplus(&both_kinds.tx_bytes, "the merged transfers");
    submit(&funder, both_kinds, "the merged transfers").await;

    assert_each_received_its_amounts(&node_url, &indexer_url, &recipients).await;

    // Each token has its own inputs and its own change, and a change output
    // that is missing is value the chain accepts and destroys. The chain
    // refuses the other dev tokens after inclusion, so this only builds.
    let balance = funder.balance().await.expect("wallet attached");
    let Some(other_token) = balance
        .shielded
        .coins
        .iter()
        .map(|coin| coin.token_type)
        .find(|token| *token != DEFAULT_SHIELDED_TOKEN)
    else {
        return common::skip("a second shielded token in the genesis wallet");
    };
    let two_tokens = funder
        .transfer_shielded_many([
            Payment::new(DEFAULT_SHIELDED_TOKEN, 1, &own_shielded),
            Payment::new(other_token, 1, &own_shielded),
            Payment::new(DEFAULT_SHIELDED_TOKEN, 1, &own_shielded),
        ])
        .build()
        .await
        .expect("build a shielded transfer of two tokens");
    assert_no_surplus(&two_tokens.tx_bytes, "the shielded transfer of two tokens");
    funder
        .release(&two_tokens.spent)
        .await
        .expect("wallet attached");
}

/// One payment of the default shielded token to each of `recipients`, of the
/// matching amount in [`AMOUNTS`].
fn shielded_payments(recipients: &[Seed]) -> Vec<Payment<ShieldedTokenType>> {
    recipients
        .iter()
        .zip(AMOUNTS)
        .map(|(seed, amount)| {
            Payment::new(
                DEFAULT_SHIELDED_TOKEN,
                amount,
                seed.shielded_address(Network::Undeployed),
            )
        })
        .collect()
}

/// One NIGHT payment to each of `recipients`, of the matching amount in
/// [`AMOUNTS`].
fn unshielded_payments(recipients: &[Seed]) -> Vec<Payment<UnshieldedTokenType>> {
    recipients
        .iter()
        .zip(AMOUNTS)
        .map(|(seed, amount)| {
            Payment::new(NIGHT, amount, seed.unshielded_address(Network::Undeployed))
        })
        .collect()
}

/// Submit `result` from `funder` and wait until the chain finalizes it.
async fn submit(funder: &MidnightProvider, result: TransferResult, what: &str) {
    funder
        .submit_reserved(result)
        .await
        .unwrap_or_else(|e| panic!("submit {what}: {e}"))
        .wait_finalized()
        .await
        .unwrap_or_else(|e| panic!("the chain applies {what}: {e}"));
}

/// Require that each of `recipients` holds one shielded coin and one NIGHT
/// UTXO, both of its own amount in [`AMOUNTS`].
async fn assert_each_received_its_amounts(node_url: &str, indexer_url: &str, recipients: &[Seed]) {
    for (seed, amount) in recipients.iter().zip(AMOUNTS) {
        let (shielded, unshielded) = received(node_url, indexer_url, seed).await;
        assert_eq!(
            shielded,
            vec![amount],
            "each recipient receives one shielded coin of its own amount"
        );
        assert_eq!(
            unshielded,
            vec![amount],
            "each recipient receives one NIGHT UTXO of its own amount"
        );
    }
}

/// Require that the proven transaction `tx_bytes` gives back every token it
/// spends but Dust: its inputs of each token add up to its outputs of that
/// token. A surplus is value the chain accepts and nobody can spend.
fn assert_no_surplus(tx_bytes: &[u8], what: &str) {
    macro_rules! deltas_in {
        ($ledger:ident) => {{
            use midnight_helpers::$ledger as l;
            let tx: l::FinalizedTransaction<l::DefaultDB> =
                l::midnight_serialize::tagged_deserialize(&mut &tx_bytes[..])
                    .expect("deserialize proven transaction");
            let mut deltas = BTreeMap::new();
            for ((token, _segment), delta) in tx.balance(None).expect("token balance") {
                if !matches!(token, l::TokenType::Dust) {
                    *deltas.entry(format!("{token:?}")).or_insert(0i128) += delta;
                }
            }
            deltas
        }};
    }
    let deltas = match LedgerVersion::of_transaction(tx_bytes).expect("a proven transaction") {
        LedgerVersion::V8 => deltas_in!(ledger_8),
        LedgerVersion::V9 => deltas_in!(ledger_9),
    };
    assert!(
        deltas.values().all(|delta| *delta == 0),
        "{what} must return the change of each token it spends, got {deltas:?}"
    );
}

/// The values of the default shielded token and of NIGHT that the wallet of
/// `seed` holds, once it holds both or a minute passes.
async fn received(node_url: &str, indexer_url: &str, seed: &Seed) -> (Vec<u128>, Vec<u128>) {
    let provider = MidnightProvider::new(node_url);
    let wallet = WalletBuilder::new(seed)
        .with_network(Network::Undeployed)
        .sync(&provider, indexer_url)
        .await
        .expect("sync the recipient");
    let provider = provider.with_wallet(wallet);

    // Finalized on chain is not yet visible through the indexer, which the
    // wallet syncs from, so poll rather than assume.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let shielded: Vec<u128> = provider
            .spendable_shielded_coins()
            .await
            .expect("wallet attached")
            .into_iter()
            .filter(|coin| coin.token_type == DEFAULT_SHIELDED_TOKEN)
            .map(|coin| coin.value)
            .collect();
        let unshielded: Vec<u128> = provider
            .unshielded_utxos()
            .await
            .expect("wallet attached")
            .into_iter()
            .filter(|utxo| utxo.token_type == NIGHT)
            .map(|utxo| utxo.value)
            .collect();
        if (!shielded.is_empty() && !unshielded.is_empty()) || Instant::now() >= deadline {
            return (shielded, unshielded);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        provider
            .resync_wallet()
            .await
            .expect("resync the recipient");
    }
}
