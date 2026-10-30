//! A transaction built with a TTL shorter than the chain's `global_ttl` lands,
//! and so does the fee intent that a sponsor adds to it.
//!
//! The build counts the TTL from the chain time of the wallet's last sync,
//! not from the Dust anchor that the wallet proves against. Each intent of the
//! transaction carries that TTL, and the chain refuses one that has passed at
//! the block that applies it. A sponsor's fee intent expires with the
//! transaction it pays for.
//!
//! Needs a devnet, and a genesis wallet with tNIGHT and dust to fund the
//! fresh address this drives.

mod common;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use midnight_provider::{
    DustlessBuilder, LedgerVersion, MidnightProvider, Network, ProviderError, Seed,
};
use midnight_wallet::{DustBalance, NIGHT, WalletError};

/// Long enough for a build, its proof and its inclusion, and well short of
/// the devnet's `global_ttl`.
const TTL: Duration = Duration::from_secs(15 * 60);

/// Enough tNIGHT that the coin carries real generationless dust.
const SEND: u128 = 5_000_000_000_000;

/// The hex of a seed no earlier run has used, because a registration is
/// permanent and the devnet keeps its chain between runs.
fn unused_seed() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nonce:064x}")
}

/// Resync `provider` until its Dust balance satisfies `done`. Finalized on
/// chain is not yet visible through the indexer, which the wallet syncs from.
async fn wait_for_dust(
    provider: &MidnightProvider,
    what: &str,
    done: impl Fn(&DustBalance) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        provider.resync_wallet().await.expect("resync");
        let dust = provider.balance().await.expect("balance").dust;
        if done(&dust) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}, got {dust:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The TTL of each intent of the proven transaction `bytes`, in seconds.
fn intent_ttls(bytes: &[u8], ledger: LedgerVersion) -> Vec<u64> {
    macro_rules! ttls {
        ($ledger:ident) => {{
            let tx: midnight_helpers::$ledger::FinalizedTransaction<midnight_helpers::DefaultDB> =
                midnight_helpers::$ledger::midnight_serialize::tagged_deserialize(&mut &bytes[..])
                    .expect("a proven transaction");
            tx.intents()
                .map(|(_, intent)| intent.ttl.to_secs())
                .collect()
        }};
    }
    match ledger {
        LedgerVersion::V8 => ttls!(ledger_8),
        LedgerVersion::V9 => ttls!(ledger_9),
    }
}

/// The time of the node's best block, in seconds.
async fn chain_now(provider: &MidnightProvider) -> u64 {
    let best = provider.best_block_hash().await.expect("best block");
    provider
        .get_block_timestamp(best)
        .await
        .expect("best block time")
        .as_secs()
}

/// Each TTL is [`TTL`] after a chain time the build read: past the chain's
/// time now, and no later than [`TTL`] after it.
fn assert_short(ttls: &[u64], now: u64, what: &str) {
    assert!(!ttls.is_empty(), "{what} carries no intent");
    for &ttl in ttls {
        assert!(
            ttl > now && ttl <= now + TTL.as_secs(),
            "{what}: an intent TTL of {ttl} is outside ({now}, {}]",
            now + TTL.as_secs()
        );
    }
}

/// The sponsored transfer has two intents, so its cost to dismiss is near the
/// chain's floor. Each further input, output or Dust spend can push it over.
/// So a fresh wallet sends its one tNIGHT UTXO whole and pays the fee with
/// its one Dust UTXO: neither coin selection nor the order of the Dust UTXOs
/// can add one.
#[tokio::test]
async fn a_short_ttl_lands_and_the_sponsor_fee_expires_with_it() {
    let Some((node_url, indexer_url)) = common::devnet_urls() else {
        return;
    };

    let funder = MidnightProvider::new(&node_url);
    let wallet = common::synced_wallet(&funder, &indexer_url, common::DEV_WALLET_SEED).await;
    let global_ttl = wallet.parameters().global_ttl.as_seconds();
    assert!(
        global_ttl > i128::from(TTL.as_secs()),
        "the devnet's global_ttl of {global_ttl}s leaves no room for a shorter TTL"
    );
    let funder = funder.with_wallet(wallet);

    let seed = unused_seed();
    let address = Seed::from_hex(&seed)
        .expect("hex seed")
        .unshielded_address(Network::Undeployed);

    // A transfer that pays its own fee: its intent and its fee intent are one.
    let paid = funder
        .transfer_unshielded(NIGHT, SEND, &address)
        .with_ttl(TTL)
        .build()
        .await
        .expect("build a transfer with a short TTL");
    assert_short(
        &intent_ttls(&paid.tx_bytes, paid.ledger_version),
        chain_now(&funder).await,
        "the self-funded transfer",
    );
    funder
        .submit_reserved(paid)
        .await
        .expect("submit the self-funded transfer")
        .wait_finalized()
        .await
        .expect("the chain applies a transaction with a short TTL");

    let fresh = common::synced_provider(&node_url, &indexer_url, &seed).await;
    wait_for_dust(
        &fresh,
        "the fresh wallet must hold the funded tNIGHT",
        |d| d.unregistered_night_utxos == 1,
    )
    .await;
    fresh
        .register_dust(None)
        .await
        .expect("build and submit the registration")
        .wait_finalized()
        .await
        .expect("registration finalized");
    wait_for_dust(&fresh, "the registered tNIGHT must generate Dust", |d| {
        d.night_generates_dust && d.spendable_utxos > 0
    })
    .await;

    // A Dustless transfer, which the same wallet then sponsors.
    let dustless = fresh
        .transfer_unshielded(NIGHT, SEND, &address)
        .with_ttl(TTL)
        .without_dust()
        .await
        .expect("build a Dustless transfer with a short TTL");
    let ledger = fresh.ledger_version().await.expect("ledger version");
    let external = intent_ttls(dustless.as_bytes(), ledger);
    // The wallet values its Dust at the time of its last Dust event, so the
    // first attempt can fall short.
    let deadline = Instant::now() + Duration::from_secs(120);
    let funded = loop {
        match fresh.balance_transaction(dustless.as_bytes()).await {
            Ok(funded) => break funded,
            Err(ProviderError::Wallet(WalletError::InsufficientDust { .. }))
                if Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => panic!("sponsor the Dustless transfer: {err}"),
        }
    };
    let merged = intent_ttls(&funded.tx_bytes, funded.ledger_version);
    assert_eq!(
        merged.len(),
        external.len() + 1,
        "the sponsor adds one fee intent"
    );
    assert!(
        merged.iter().all(|ttl| external.contains(ttl)),
        "the fee intent must expire with the transaction it pays for: \
         external {external:?}, merged {merged:?}"
    );
    assert_short(&merged, chain_now(&fresh).await, "the sponsored transfer");
    fresh
        .submit_reserved(funded)
        .await
        .expect("submit the sponsored transfer")
        .wait_finalized()
        .await
        .expect("the chain applies a sponsor fee intent with the short TTL");
}
