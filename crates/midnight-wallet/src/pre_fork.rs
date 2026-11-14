//! The reader of the shielded events that a chain made before its hard fork
//! to ledger 9.
//!
//! A chain that crossed the fork serves its ledger 8 events before its ledger
//! 9 events, in one zswap stream. The coins that a wallet received before the
//! fork exist only in those ledger 8 events, so a replay from the first event
//! reads them here and carries the state across the fork.

use midnight_helpers::fork::old_to_new_ser;
use midnight_helpers::ledger_8::mn_ledger::semantics::ZswapLocalStateExt;
use midnight_helpers::midnight_serialize::tagged_deserialize;
use midnight_helpers::{DefaultDB, ledger_8};
use midnight_indexer_client::SubscriptionClient;
use tokio::sync::mpsc;

use crate::generation::Generation;
use crate::replay::{LedgerStream, replay_ledger_events};
use crate::{CoinInfo, SyncProgress, WalletError};

/// The shielded state that the events before the fork leave, in ledger 9.
pub(crate) struct PreFork {
    pub(crate) state: midnight_helpers::WalletState<DefaultDB>,
    /// The id of the last event that the reader applied, or 0 when it applied
    /// none.
    pub(crate) last_id: i64,
    /// The id of the first ledger 9 event, which the reader did not apply, or
    /// `None` when the stream ended before one.
    pub(crate) fork_id: Option<i64>,
}

/// Replay the zswap stream of the wallet whose ledger 8 keys are
/// `secret_keys` from its first event up to the first ledger 9 event, and
/// claim `coins` on the way.
///
/// The replay hands back the state in ledger 9 also when the stream ends
/// before a ledger 9 event: the chain runs ledger 9, so the replay goes on
/// in ledger 9. It sends the [`SyncProgress::ZswapEvents`] progress of the
/// events it reads, and writes nothing to disk.
pub(crate) async fn replay(
    sub_client: &SubscriptionClient,
    secret_keys: &ledger_8::SecretKeys,
    coins: &[CoinInfo],
    progress: Option<mpsc::Sender<SyncProgress>>,
) -> Result<PreFork, WalletError> {
    let coin_public_key = secret_keys.coin_public_key();
    let mut state = coins
        .iter()
        .fold(ledger_8::WalletState::new(), |state, coin| {
            state.watch_for(&coin_public_key, &coin_info(coin))
        });
    let replay = replay_ledger_events(
        sub_client,
        LedgerStream::Zswap,
        Generation::V8,
        0,
        false,
        progress,
        |id, raw, _| {
            let event: ledger_8::Event<DefaultDB> = tagged_deserialize(raw).map_err(|e| {
                WalletError::Sync(format!("deserialize ledger 8 zswap event id={id}: {e}"))
            })?;
            state = state.replay_events(secret_keys, [&event]).map_err(|e| {
                WalletError::Sync(format!("replay ledger 8 zswap event id={id}: {e}"))
            })?;
            Ok(())
        },
    )
    .await?;

    let state = old_to_new_ser(&state)
        .map_err(|e| WalletError::Sync(format!("carry the shielded state across the fork: {e}")))?;
    Ok(PreFork {
        state,
        last_id: replay.last_id,
        fork_id: replay.later_ledger.map(|(id, _)| id),
    })
}

fn coin_info(coin: &CoinInfo) -> ledger_8::CoinInfo {
    ledger_8::CoinInfo {
        nonce: ledger_8::Nonce(coin.nonce.0),
        type_: ledger_8::ShieldedTokenType(coin.type_.0),
        value: coin.value,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use midnight_helpers::ledger_8::mn_ledger::events::{
        Event, EventDetails, EventSource, ZswapPreimageEvidence,
    };
    use midnight_helpers::ledger_8::{HashOutput, Recipient, TransactionHash};
    use midnight_helpers::midnight_serialize::tagged_serialize;

    use super::*;
    use crate::Seed;
    use crate::keys::RoleKeys;

    /// The hex of a ledger 8 event for an output of `coin` to `seed`'s wallet
    /// that carries no discovery ciphertext. Only a registration claims it.
    pub(crate) fn ciphertextless_output(seed: &Seed, coin: &CoinInfo, mt_index: u64) -> String {
        let commitment = coin_info(coin).commitment(&Recipient::User(
            RoleKeys::derive(seed).zswap_pre_fork.coin_public_key(),
        ));
        let event: Event<DefaultDB> = Event {
            source: EventSource {
                transaction_hash: TransactionHash(HashOutput([0u8; 32])),
                logical_segment: 0,
                physical_segment: 0,
            },
            content: EventDetails::ZswapOutput {
                commitment,
                preimage_evidence: ZswapPreimageEvidence::None,
                contract: None,
                mt_index,
            },
        };
        let mut raw = Vec::new();
        tagged_serialize(&event, &mut raw).unwrap();
        hex::encode(raw)
    }
}
