//! A wallet's state files in this generation's encoding, under the snapshot
//! layout of [`crate::storage`].

use std::path::Path;

use tracing::info;

use super::helpers::{DefaultDB, DustWallet, WalletState as ZswapLocalState};
use crate::WalletError;
use crate::chain_pin::ChainPin;
use crate::storage::{
    StoredMetadata, StoredUtxo, dust_wallet_file, read_metadata, storage_dir, tagged_from_file,
    tagged_to_file, zswap_file,
};
use midnight_types::TrackedUtxo;

pub(crate) struct LoadedState {
    pub zswap_state: ZswapLocalState<DefaultDB>,
    pub dust_wallet: DustWallet<DefaultDB>,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
    pub last_tx_id: Option<i64>,
    pub chain_pin: Option<ChainPin>,
    pub unshielded_utxos: Vec<TrackedUtxo>,
}

pub(crate) fn load(
    base: &Path,
    network: &str,
    wallet_id: &str,
) -> Result<Option<LoadedState>, WalletError> {
    let dir = storage_dir(base, network, wallet_id);
    // No identity check here: the directory name is derived from the wallet's
    // public id, so reaching a metadata file already means it is this wallet's.
    let Some(metadata) = read_metadata(&dir)? else {
        return Ok(None);
    };
    if metadata.ledger_version != super::LEDGER {
        return Err(WalletError::LedgerMismatch {
            expected: super::LEDGER,
            found: metadata.ledger_version,
        });
    }

    let zswap_state = tagged_from_file(&dir, &zswap_file(metadata.generation))?;
    let dust_wallet = tagged_from_file(&dir, &dust_wallet_file(metadata.generation))?;

    let unshielded_utxos: Vec<TrackedUtxo> = metadata
        .unshielded_utxos
        .into_iter()
        .map(TrackedUtxo::try_from)
        .collect::<Result<_, _>>()?;

    info!(
        zswap_event_id = metadata.zswap_event_id,
        dust_event_id = metadata.dust_event_id,
        unshielded_utxos = unshielded_utxos.len(),
        "loaded wallet state from disk"
    );

    Ok(Some(LoadedState {
        zswap_state,
        dust_wallet,
        zswap_event_id: metadata.zswap_event_id,
        dust_event_id: metadata.dust_event_id,
        last_block_height: metadata.last_block_height,
        last_tx_id: metadata.last_tx_id,
        chain_pin: metadata.chain_pin,
        unshielded_utxos,
    }))
}

/// Everything one snapshot records, mirroring [`LoadedState`] on the way out.
///
/// A struct rather than a parameter list, so adding a field to a snapshot does
/// not lengthen a positional call that two sites have to keep in the same
/// order.
pub(crate) struct Snapshot<'a> {
    pub zswap_state: &'a ZswapLocalState<DefaultDB>,
    pub dust_wallet: &'a DustWallet<DefaultDB>,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
    pub last_tx_id: Option<i64>,
    pub chain_pin: Option<&'a ChainPin>,
    pub unshielded_utxos: &'a [TrackedUtxo],
}

pub(crate) fn save(
    base: &Path,
    network: &str,
    wallet_id: &str,
    snapshot: Snapshot<'_>,
) -> Result<(), WalletError> {
    let Snapshot {
        zswap_state,
        dust_wallet,
        zswap_event_id,
        dust_event_id,
        last_block_height,
        last_tx_id,
        chain_pin,
        unshielded_utxos,
    } = snapshot;
    let metadata = StoredMetadata {
        generation: 0,
        ledger_version: super::LEDGER,
        zswap_event_id,
        dust_event_id,
        last_block_height,
        last_tx_id,
        chain_pin: chain_pin.cloned(),
        unshielded_utxos: unshielded_utxos.iter().map(StoredUtxo::from).collect(),
    };
    crate::storage::save_snapshot(base, network, wallet_id, metadata, |dir, generation| {
        tagged_to_file(dir, &zswap_file(generation), zswap_state)?;
        tagged_to_file(dir, &dust_wallet_file(generation), dust_wallet)
    })
}
