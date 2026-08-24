//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_provider::MidnightProvider;
use midnight_wallet::WalletSeed;
use midnight_wallet::{LocalWallet, Wallet};

const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn dev_seed() -> WalletSeed {
    WalletSeed::try_from_hex_str(DEV_SEED).unwrap()
}

fn node_url() -> Option<String> {
    std::env::var("MIDNIGHT_NODE_URL").ok()
}

fn indexer_url() -> Option<String> {
    std::env::var("MIDNIGHT_INDEXER_URL").ok()
}

macro_rules! require_devnet {
    () => {{
        let node = match node_url() {
            Some(u) => u,
            None => {
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_NODE_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_NODE_URL not set");
                return;
            }
        };
        let indexer = match indexer_url() {
            Some(u) => u,
            None => {
                if std::env::var("MIDNIGHT_E2E").is_ok() {
                    panic!("MIDNIGHT_INDEXER_URL must be set in CI");
                }
                eprintln!("skipping: MIDNIGHT_INDEXER_URL not set");
                return;
            }
        };
        (node, indexer)
    }};
}

// ---------------------------------------------------------------------------
// Indexer-based sync (zswap + dust + unshielded events)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_replays_events() {
    let (node, indexer) = require_devnet!();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        dev_seed(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let cursors = provider
        .sync_cursors()
        .await
        .expect("wallet attached after sync_wallet");
    let utxos = provider.unshielded_utxos().await.expect("wallet attached");
    eprintln!(
        "synced: height={}, utxos={}, zswap_event_id={}, dust_event_id={}",
        cursors.last_block_height,
        utxos.len(),
        cursors.zswap_event_id,
        cursors.dust_event_id,
    );

    assert!(
        cursors.last_tx_id.is_some(),
        "expected last_tx_id to be set after sync"
    );
    assert!(
        cursors.zswap_event_id > 0,
        "expected zswap events to have been replayed"
    );
    assert!(
        cursors.dust_event_id > 0,
        "expected dust events to have been replayed"
    );
}

// ---------------------------------------------------------------------------
// Build context from indexed state via the provider
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_build_context_succeeds() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let context = provider
        .build_context()
        .await
        .expect("build_context should succeed");

    let wallets = context.wallets.lock().unwrap();
    assert!(
        wallets.contains_key(&seed),
        "context should contain our wallet"
    );
}

// ---------------------------------------------------------------------------
// Transfer transaction building via the provider's wallet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_shielded_transfer() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let balance = provider
        .balance()
        .await
        .expect("wallet attached after sync_wallet");
    eprintln!(
        "pre-transfer balance: dust={}, shielded={}",
        balance.dust.spendable_utxos, balance.shielded.total_count,
    );

    let recipient =
        midnight_wallet::address::derive_shielded(&seed, midnight_wallet::Network::Undeployed);
    // Submit and finalize so subsequent tests don't try to double-spend the
    // same dust UTXOs.
    let pending = provider
        .transfer_shielded(
            midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32])),
            1,
            &recipient,
        )
        .await
        .expect("shielded transfer should build + submit successfully");
    eprintln!("transaction submitted: {}", pending.extrinsic_hash_hex());
    let (_best, pending) = pending.wait_best().await.expect("wait_best");
    let (_finalized, _) = pending.wait_finalized().await.expect("wait_finalized");
    eprintln!("transaction finalized");
}

/// Exercises the shielded transfer build path with a non-zero shielded token
/// id. The existing `build_shielded_transfer` uses the all-zero token id
/// `[0; 32]`, which is just the conventional default the dev preset mints; a
/// future change that quietly short-circuits coin selection for that default
/// would still pass that test. This test picks a different shielded token at
/// runtime (the dev preset mints a few) and asserts the build path handles
/// it identically. Skips if only the zero-id token is held.
///
/// (NIGHT is the chain's native *unshielded* token and lives in
/// `WalletBalance::unshielded`; there is no shielded NIGHT, so the property
/// here is purely about token-id genericity in the shielded path.)
///
/// We deliberately stop at build (no submit) for two reasons: (a) the
/// pre-allocated non-default dev tokens have chain-side transfer restrictions
/// (`OutOfDustValidityWindow`, code 171) so the chain will reject the tx after
/// inclusion, and (b) submitting would pollute the mempool with dust spends
/// that conflict with `build_shielded_transfer` running in parallel. Build
/// success — proof generation, offer construction, and tagged serialization
/// — is enough to pin the property.
#[tokio::test]
async fn build_shielded_transfer_arbitrary_token_id() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let zero_token = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    let balance = provider
        .balance()
        .await
        .expect("wallet attached after sync_wallet");
    let Some(coin) = balance
        .shielded
        .coins
        .iter()
        .find(|c| c.token_type != zero_token)
        .cloned()
    else {
        eprintln!("skipping: dev wallet has no shielded coins with a non-zero token id");
        return;
    };
    eprintln!(
        "shielded coin with non-zero token id: token={} value={}",
        coin.token_type_hex(),
        coin.value
    );

    let recipient =
        midnight_wallet::address::derive_shielded(&seed, midnight_wallet::Network::Undeployed);
    let tx_result = provider
        .transfer_shielded(coin.token_type, 1, &recipient)
        .build()
        .await
        .expect("shielded transfer of arbitrary token id should build (proofs + serialize)");
    eprintln!(
        "shielded transfer built, tx_bytes={}",
        tx_result.tx_bytes.len()
    );
    assert!(
        tx_result.tx_bytes.len() > 1000,
        "tx bytes too small to be a real proven shielded transfer ({})",
        tx_result.tx_bytes.len()
    );
}

/// Conservation says the offer balances; it says nothing about who ends up
/// holding what. `Transaction::balance` sums signed per-token deltas and never
/// looks at a recipient key, so a build that swapped the payment and the change
/// would satisfy every other assertion in this file. Pay a wallet that holds
/// nothing else and require it to receive exactly the requested amount.
#[tokio::test]
async fn shielded_transfer_pays_the_recipient_the_requested_amount() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    // A seed used by nothing else, so any coin it holds came from this test.
    let recipient_seed = WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000042",
    )
    .unwrap();
    let recipient = midnight_wallet::address::derive_shielded(
        &recipient_seed,
        midnight_wallet::Network::Undeployed,
    );

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed,
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let token = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    // Distinctive enough that a coin of this value is unambiguous, and far
    // below any single dev coin so the sender must return change.
    const AMOUNT: u128 = 7;

    let payee = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        payee.indexer_url(),
        recipient_seed,
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("payee sync should succeed");
    let payee = payee.with_wallet(LocalWallet::new(wallet));
    let before: std::collections::HashSet<_> = payee
        .spendable_shielded_coins()
        .await
        .expect("payee coins")
        .into_iter()
        .map(|c| c.nullifier)
        .collect();

    let pending = provider
        .transfer_shielded(token, AMOUNT, &recipient)
        .await
        .expect("shielded transfer should build + submit");
    let (_best, pending) = pending.wait_best().await.expect("wait_best");
    let (_finalized, _) = pending.wait_finalized().await.expect("wait_finalized");

    // The payee learns of the coin through its discovery ciphertext, so wait
    // for the indexer to replay the transaction rather than assuming.
    let mut received = None;
    for _ in 0..20 {
        payee.resync_wallet().await.expect("payee resync");
        received = payee
            .spendable_shielded_coins()
            .await
            .expect("payee coins")
            .into_iter()
            .find(|c| c.token_type == token && !before.contains(&c.nullifier));
        if received.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }

    let received = received.expect("payee must receive a new coin of the transferred token");
    assert_eq!(
        received.value, AMOUNT,
        "the recipient must receive the requested amount, not the sender's change"
    );
}

/// Sum the signed deltas a proven transaction carries for one shielded token.
fn shielded_delta(tx_bytes: &[u8], token: midnight_helpers::ShieldedTokenType) -> i128 {
    let tx: midnight_helpers::FinalizedTransaction<midnight_helpers::DefaultDB> =
        midnight_helpers::midnight_serialize::tagged_deserialize(&mut &tx_bytes[..])
            .expect("deserialize proven transaction");
    tx.balance(None)
        .expect("token balance")
        .iter()
        .filter(
            |((tt, _seg), _)| matches!(tt, midnight_helpers::TokenType::Shielded(s) if *s == token),
        )
        .map(|(_, v)| *v)
        .sum()
}

/// A self-funded transfer must conserve the transferred token: whatever the
/// selected coins are worth beyond `amount` has to come back as change, leaving
/// a zero delta. A positive delta is value the transaction destroys, since the
/// chain rejects only a negative balance and nobody can ever spend a surplus.
///
/// This is the property that makes a partial spend of an over-large coin safe,
/// so it is asserted at build time rather than through a balance comparison,
/// which sync timing would make flaky.
#[tokio::test]
async fn shielded_transfer_conserves_value() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let token = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    let recipient =
        midnight_wallet::address::derive_shielded(&seed, midnight_wallet::Network::Undeployed);

    // 1 is deliberately far below any coin the dev preset mints, so the
    // remainder is large and an abandoned one would be unmissable.
    let result = provider
        .transfer_shielded(token, 1, &recipient)
        .build()
        .await
        .expect("shielded transfer should build");

    assert_eq!(
        shielded_delta(&result.tx_bytes, token),
        0,
        "a self-funded transfer must leave no surplus; the remainder of the \
         selected coins has to return as change"
    );
    assert!(
        !result.spent_shielded_inputs.is_empty(),
        "the build selects coins up front, so it must surface their nullifiers \
         for reservation"
    );
}

/// A payment is bounded by the wallet's balance in a token, not by its largest
/// single coin. Selecting one coin that alone covers the amount caps transfers
/// at the biggest coin and fails a fragmented wallet outright, so spend an
/// amount no single coin can cover and require several inputs.
#[tokio::test]
async fn shielded_transfer_spans_multiple_coins() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let token = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    let balance = provider
        .balance()
        .await
        .expect("wallet attached after sync_wallet");
    let values: Vec<u128> = balance
        .shielded
        .coins
        .iter()
        .filter(|c| c.token_type == token)
        .map(|c| c.value)
        .collect();

    let (Some(largest), total) = (
        values.iter().copied().max(),
        values.iter().copied().sum::<u128>(),
    ) else {
        eprintln!("skipping: dev wallet holds no coins of the default shielded token");
        return;
    };
    if total <= largest {
        eprintln!("skipping: dev wallet holds only one coin of the default shielded token");
        return;
    }

    // One more than the biggest coin: satisfiable only by combining coins.
    let amount = largest + 1;
    let recipient =
        midnight_wallet::address::derive_shielded(&seed, midnight_wallet::Network::Undeployed);
    let result = provider
        .transfer_shielded(token, amount, &recipient)
        .build()
        .await
        .unwrap_or_else(|e| {
            panic!(
                "transfer of {amount} across {} coins should build: {e}",
                values.len()
            )
        });

    assert!(
        result.spent_shielded_inputs.len() >= 2,
        "covering {amount} needs more than the largest coin ({largest}), so at \
         least two inputs; got {}",
        result.spent_shielded_inputs.len()
    );
    assert_eq!(
        shielded_delta(&result.tx_bytes, token),
        0,
        "a multi-coin transfer must return its change too"
    );
}

/// The swap-half builder must produce exactly the unbalanced offer a native
/// two-party swap needs: a give-side surplus of `+give_amount` and a
/// receive-side deficit of `-receive_amount`. Critically, the give delta must
/// be the amount *given*, not the full value of the coins selected. Asserting
/// it pins the change handling too (mishandled change would inflate the give
/// delta past `give_amount`).
///
/// Builds against the dev preset's native shielded token (give side, which the
/// wallet holds) and an arbitrary distinct receive-side token id. The half only
/// creates an output for the receive token, so the wallet need not hold any of
/// it. Fee-less by construction: no funding seed, so no Dust intent rides the
/// transaction and the receive deficit is the only shortfall. Stops at build
/// (the half is not submittable on its own).
#[tokio::test]
async fn build_shielded_swap_half_has_mirror_deltas() {
    let (node, indexer) = require_devnet!();
    let seed = dev_seed();

    let provider = MidnightProvider::new(&node, &indexer).expect("provider construction");
    let wallet = Wallet::sync(
        provider.indexer_url(),
        seed.clone(),
        midnight_wallet::Network::Undeployed,
    )
    .await
    .expect("indexer sync should succeed");
    let provider = provider.with_wallet(LocalWallet::new(wallet));

    let give_token = midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0u8; 32]));
    let receive_token =
        midnight_helpers::ShieldedTokenType(midnight_helpers::HashOutput([0xABu8; 32]));
    const GIVE: u128 = 1;
    const RECEIVE: u128 = 3;

    let result = provider
        .shielded_swap(give_token, GIVE, receive_token, RECEIVE)
        .build()
        .await
        .expect("swap half should build (coin selection + proofs + serialize)");

    // The build selects concrete give-side coins up front and surfaces their
    // nullifiers so the caller can reserve them (unlike a plain transfer, whose
    // coins the ledger selects internally).
    assert!(
        !result.spent_shielded_inputs.is_empty(),
        "swap half should surface the spent give-side coin nullifiers"
    );
    assert!(
        result.tx_bytes.len() > 1000,
        "tx bytes too small to be a real proven offer ({})",
        result.tx_bytes.len()
    );

    let tx: midnight_helpers::FinalizedTransaction<midnight_helpers::DefaultDB> =
        midnight_helpers::midnight_serialize::tagged_deserialize(&mut &result.tx_bytes[..])
            .expect("deserialize proven swap half");
    let balance = tx.balance(None).expect("token balance");

    let delta = |token: midnight_helpers::ShieldedTokenType| -> i128 {
        balance
            .iter()
            .filter(|((tt, _seg), _)| {
                matches!(tt, midnight_helpers::TokenType::Shielded(s) if *s == token)
            })
            .map(|(_, v)| *v)
            .sum()
    };

    assert_eq!(
        delta(give_token),
        GIVE as i128,
        "give-side delta must be +give_amount (change handled), balance {balance:?}"
    );
    assert_eq!(
        delta(receive_token),
        -(RECEIVE as i128),
        "receive-side delta must be -receive_amount, balance {balance:?}"
    );
}

// ---------------------------------------------------------------------------
// Subscription client connectivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_client_connects() {
    let (_node, indexer) = require_devnet!();

    let sub_client = midnight_indexer_client::SubscriptionClient::new(&indexer);

    let variables = serde_json::json!({ "offset": { "height": 0 } });
    let mut subscription = sub_client
        .subscribe::<serde_json::Value>(
            midnight_indexer_client::subscription::queries::BLOCKS_SUBSCRIPTION,
            variables,
        )
        .await
        .expect("blocks subscription should connect");

    let event = tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
        .await
        .expect("should receive block within 10s");

    assert!(
        event.is_some(),
        "subscription should yield at least one event"
    );
    let event = event.unwrap().expect("event should be Ok");
    eprintln!("received block event: {event}");
}
