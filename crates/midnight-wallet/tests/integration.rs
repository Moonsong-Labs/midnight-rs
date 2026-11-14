//! Integration tests for midnight-wallet against a running devnet.
//!
//! These tests require MIDNIGHT_NODE_URL and MIDNIGHT_INDEXER_URL to be set.
//! The CI runs a devnet (node + indexer) via docker compose.
//!
//! Run locally:
//!   MIDNIGHT_NODE_URL=ws://127.0.0.1:9944 MIDNIGHT_INDEXER_URL=http://127.0.0.1:8088 \
//!     cargo test -p midnight-wallet --test integration -- --show-output

use midnight_provider::{MidnightProvider, TransactionRequest};
use midnight_wallet::{Seed, Wallet};

const DEFAULT_SHIELDED_TOKEN: midnight_wallet::ShieldedTokenType =
    midnight_wallet::ShieldedTokenType(midnight_wallet::HashOutput([0; 32]));

const DEV_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// A seed that nothing else uses, so any coin its wallet holds came from a
/// test in this file.
const RECIPIENT_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000042";

async fn synced_wallet(provider: &MidnightProvider, indexer: &str, seed: &str) -> Wallet {
    Wallet::from_seed(seed.parse::<Seed>().expect("hex seed"))
        .with_network(midnight_wallet::Network::Undeployed)
        .sync(provider, indexer)
        .await
        .expect("indexer sync should succeed")
}

async fn synced_provider(node: &str, indexer: &str, seed: &str) -> MidnightProvider<Wallet> {
    let provider = MidnightProvider::new(node);
    let wallet = synced_wallet(&provider, indexer, seed).await;
    provider.with_wallet(wallet)
}

/// Prints a skip line on stderr for the precondition `missing`. The caller
/// then returns without running the test.
///
/// # Panics
///
/// When `MIDNIGHT_E2E` is set. The make targets that run the devnet tests set
/// it, so a test that they run never skips.
fn skip(missing: &str) {
    if std::env::var_os("MIDNIGHT_E2E").is_some() {
        panic!("{missing} is missing while MIDNIGHT_E2E is set");
    }
    eprintln!("skipping: {missing} is missing");
}

fn devnet_urls() -> Option<(String, String)> {
    let (Ok(node), Ok(indexer)) = (
        std::env::var("MIDNIGHT_NODE_URL"),
        std::env::var("MIDNIGHT_INDEXER_URL"),
    ) else {
        skip("MIDNIGHT_NODE_URL or MIDNIGHT_INDEXER_URL");
        return None;
    };
    Some((node, indexer))
}

// ---------------------------------------------------------------------------
// Transfer transaction building via the provider's wallet
// ---------------------------------------------------------------------------

/// Exercises the shielded transfer build path with a non-zero shielded token
/// id. The other shielded transfer tests in this file use the all-zero token
/// id `[0; 32]`, which is only the default that the dev preset mints. A change
/// that short-circuits coin selection for that default passes them. This test
/// picks a different shielded token at runtime. The dev preset mints a few. The
/// test requires the build to conserve that token, as it does the default one.
///
/// (NIGHT is the chain's native *unshielded* token and lives in
/// `WalletBalance::unshielded`; there is no shielded NIGHT, so the property
/// here is purely about token-id genericity in the shielded path.)
///
/// The test stops at build and does not submit, for two reasons. First, the
/// chain rejects a transfer of the pre-allocated non-default dev tokens after
/// inclusion (`OutOfDustValidityWindow`, code 171). Second, a submit adds Dust
/// spends to the mempool, and these can conflict with the transfers that the
/// other tests in this file submit.
#[tokio::test]
async fn build_shielded_transfer_arbitrary_token_id() {
    let Some((node, indexer)) = devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node);
    let wallet = synced_wallet(&provider, &indexer, DEV_SEED).await;
    let recipient = wallet.shielded_recipient();
    let provider = provider.with_wallet(wallet);

    let balance = provider.wallet().balance().await;
    let Some(coin) = balance
        .shielded
        .coins
        .iter()
        .find(|c| c.token_type != DEFAULT_SHIELDED_TOKEN)
        .cloned()
    else {
        return skip("a shielded coin with a non-zero token id");
    };
    eprintln!(
        "shielded coin with non-zero token id: token={} value={}",
        coin.token_type_hex(),
        coin.value
    );

    let tx_bytes = provider
        .fill(TransactionRequest::shielded(coin.token_type, 1, recipient))
        .await
        .expect("shielded transfer of arbitrary token id should build (proofs + serialize)")
        .into_bytes();
    eprintln!("shielded transfer built, tx_bytes={}", tx_bytes.len());
    assert_eq!(
        shielded_delta(&tx_bytes, coin.token_type),
        0,
        "a self-funded transfer of a non-zero token id must return its change \
         in that token"
    );
    // The bytes keep the reservation, and a read of the wallet does not sync.
    let spent = spent_nullifiers(&tx_bytes);
    let spendable = provider.wallet().spendable_shielded_coins().await;
    assert!(
        !spent.is_empty()
            && spendable
                .iter()
                .all(|coin| !spent.contains(&coin.nullifier)),
        "the build must reserve the coins it spends"
    );
}

/// Conservation says the offer balances; it says nothing about who ends up
/// holding what. `Transaction::balance` sums signed per-token deltas and never
/// looks at a recipient key, so a build that swapped the payment and the change
/// would satisfy every other assertion in this file. Pay a wallet that holds
/// nothing else and require it to receive exactly the requested amount.
#[tokio::test]
async fn shielded_transfer_pays_the_recipient_the_requested_amount() {
    let Some((node, indexer)) = devnet_urls() else {
        return;
    };

    let provider = synced_provider(&node, &indexer, DEV_SEED).await;

    // Distinctive enough that a coin of this value is unambiguous, and far
    // below any single dev coin so the sender must return change.
    const AMOUNT: u128 = 7;

    let payee = MidnightProvider::new(&node);
    let wallet = synced_wallet(&payee, &indexer, RECIPIENT_SEED).await;
    let recipient = wallet.shielded_recipient();
    let payee = payee.with_wallet(wallet);
    let before: std::collections::HashSet<_> = payee
        .wallet()
        .spendable_shielded_coins()
        .await
        .into_iter()
        .map(|c| c.nullifier)
        .collect();

    let pending = provider
        .send_transaction(TransactionRequest::shielded(
            DEFAULT_SHIELDED_TOKEN,
            AMOUNT,
            recipient,
        ))
        .await
        .expect("shielded transfer should build + submit");
    pending.get_receipt().await.expect("get_receipt");

    // The payee learns of the coin through its discovery ciphertext, so wait
    // for the indexer to replay the transaction rather than assuming.
    let mut received = None;
    for _ in 0..20 {
        payee.wallet().sync(&payee).await.expect("payee resync");
        received = payee
            .wallet()
            .spendable_shielded_coins()
            .await
            .into_iter()
            .find(|c| c.token_type == DEFAULT_SHIELDED_TOKEN && !before.contains(&c.nullifier));
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

/// The nullifiers of the shielded inputs of a proven transaction.
fn spent_nullifiers(tx_bytes: &[u8]) -> Vec<midnight_wallet::Nullifier> {
    use midnight_helpers::midnight_serialize::tagged_deserialize;
    use midnight_helpers::{DefaultDB, FinalizedTransaction, Transaction};
    use midnight_types::convert::IntoSdk;
    let tx: FinalizedTransaction<DefaultDB> =
        tagged_deserialize(&mut &tx_bytes[..]).expect("deserialize proven transaction");
    let Transaction::Standard(tx) = tx else {
        panic!("a transfer is a standard transaction");
    };
    tx.inputs()
        .map(|input| input.nullifier.into_sdk())
        .collect()
}

/// Sum the signed deltas a proven transaction carries for one shielded token.
fn shielded_delta(tx_bytes: &[u8], token: midnight_wallet::ShieldedTokenType) -> i128 {
    use midnight_helpers::midnight_serialize::tagged_deserialize;
    use midnight_helpers::{DefaultDB, FinalizedTransaction, TokenType};
    let tx: FinalizedTransaction<DefaultDB> =
        tagged_deserialize(&mut &tx_bytes[..]).expect("deserialize proven transaction");
    tx.balance(None)
        .expect("token balance")
        .iter()
        .filter(|((tt, _seg), _)| matches!(tt, TokenType::Shielded(s) if s.0 == token.0))
        .map(|(_, v)| *v)
        .sum()
}

/// A payment is bounded by the wallet's balance in a token, not by its largest
/// single coin. Selecting one coin that alone covers the amount caps transfers
/// at the biggest coin and fails a fragmented wallet outright, so spend an
/// amount no single coin can cover and require several inputs.
#[tokio::test]
async fn shielded_transfer_spans_multiple_coins() {
    let Some((node, indexer)) = devnet_urls() else {
        return;
    };

    let provider = MidnightProvider::new(&node);
    let wallet = synced_wallet(&provider, &indexer, DEV_SEED).await;
    let recipient = wallet.shielded_recipient();
    let provider = provider.with_wallet(wallet);

    let balance = provider.wallet().balance().await;
    let values: Vec<u128> = balance
        .shielded
        .coins
        .iter()
        .filter(|c| c.token_type == DEFAULT_SHIELDED_TOKEN)
        .map(|c| c.value)
        .collect();

    let (Some(largest), total) = (
        values.iter().copied().max(),
        values.iter().copied().sum::<u128>(),
    ) else {
        return skip("a coin of the default shielded token");
    };
    if total <= largest {
        return skip("a second coin of the default shielded token");
    }

    // One more than the biggest coin: satisfiable only by combining coins.
    let amount = largest + 1;
    let tx_bytes = provider
        .fill(TransactionRequest::shielded(
            DEFAULT_SHIELDED_TOKEN,
            amount,
            recipient,
        ))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "transfer of {amount} across {} coins should build: {e}",
                values.len()
            )
        })
        .into_bytes();

    let inputs = spent_nullifiers(&tx_bytes).len();
    assert!(
        inputs >= 2,
        "covering {amount} needs more than the largest coin ({largest}), so at \
         least two inputs; got {inputs}"
    );
    assert_eq!(
        shielded_delta(&tx_bytes, DEFAULT_SHIELDED_TOKEN),
        0,
        "a multi-coin transfer must return its change too"
    );
}
