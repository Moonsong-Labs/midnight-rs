use std::path::{Path, PathBuf};

use midnight_node_ledger_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_node_ledger_helpers::{
    DefaultDB, DustWallet, WalletSeed, WalletState as ZswapLocalState,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::WalletError;
use crate::state::TrackedUtxo;

const METADATA_FILE: &str = "metadata.json";
const ZSWAP_FILE: &str = "zswap.bin";
const DUST_WALLET_FILE: &str = "dust_wallet.bin";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMetadata {
    seed_hex: String,
    zswap_event_id: i64,
    dust_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: Vec<StoredUtxo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredUtxo {
    owner: String,
    token_type: String,
    value: String,
    intent_hash: Option<String>,
    output_index: Option<i64>,
}

impl From<&TrackedUtxo> for StoredUtxo {
    fn from(u: &TrackedUtxo) -> Self {
        Self {
            owner: u.owner.clone(),
            token_type: u.token_type.clone(),
            value: u.value.to_string(),
            intent_hash: u.intent_hash.clone(),
            output_index: u.output_index,
        }
    }
}

impl TryFrom<StoredUtxo> for TrackedUtxo {
    type Error = WalletError;

    fn try_from(u: StoredUtxo) -> Result<Self, Self::Error> {
        let value: u128 = u.value.parse().map_err(|e| {
            WalletError::Storage(format!(
                "failed to parse stored UTXO value '{}': {e}",
                u.value
            ))
        })?;
        Ok(Self {
            owner: u.owner,
            token_type: u.token_type,
            value,
            intent_hash: u.intent_hash,
            output_index: u.output_index,
        })
    }
}

fn storage_dir(base: &Path, network: &str, seed: &WalletSeed) -> PathBuf {
    let prefix = hex::encode(&seed.as_bytes()[..4]);
    base.join(network).join(prefix)
}

fn tagged_to_file<
    T: midnight_node_ledger_helpers::midnight_serialize::Serializable
        + midnight_node_ledger_helpers::midnight_serialize::Tagged,
>(
    dir: &Path,
    filename: &str,
    value: &T,
) -> Result<(), WalletError> {
    let path = dir.join(filename);
    let tmp = dir.join(format!("{filename}.tmp"));
    let mut buf = Vec::new();
    tagged_serialize(value, &mut buf)
        .map_err(|e| WalletError::Storage(format!("serialize {filename}: {e}")))?;
    std::fs::write(&tmp, &buf)
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| WalletError::Storage(format!("rename {filename}: {e}")))?;
    Ok(())
}

fn tagged_from_file<
    T: midnight_node_ledger_helpers::midnight_serialize::Deserializable
        + midnight_node_ledger_helpers::midnight_serialize::Tagged,
>(
    dir: &Path,
    filename: &str,
) -> Result<T, WalletError> {
    let path = dir.join(filename);
    let bytes = std::fs::read(&path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", path.display())))?;
    tagged_deserialize(&bytes[..])
        .map_err(|e| WalletError::Storage(format!("deserialize {filename}: {e}")))
}

pub(crate) struct LoadedState {
    pub zswap_state: ZswapLocalState<DefaultDB>,
    pub dust_wallet: DustWallet<DefaultDB>,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_tx_id: Option<i64>,
    pub unshielded_utxos: Vec<TrackedUtxo>,
}

pub(crate) fn load(
    base: &Path,
    network: &str,
    seed: &WalletSeed,
) -> Result<Option<LoadedState>, WalletError> {
    let dir = storage_dir(base, network, seed);
    let meta_path = dir.join(METADATA_FILE);

    if !meta_path.exists() {
        return Ok(None);
    }

    let meta_json = std::fs::read_to_string(&meta_path)
        .map_err(|e| WalletError::Storage(format!("read {}: {e}", meta_path.display())))?;
    let metadata: StoredMetadata = serde_json::from_str(&meta_json)
        .map_err(|e| WalletError::Storage(format!("parse metadata: {e}")))?;

    let stored_seed = WalletSeed::try_from_hex_str(&metadata.seed_hex)
        .map_err(|e| WalletError::Storage(format!("parse stored seed: {e}")))?;
    if stored_seed != *seed {
        return Err(WalletError::Storage(
            "stored seed does not match requested seed".into(),
        ));
    }

    let zswap_state = tagged_from_file(&dir, ZSWAP_FILE)?;
    let dust_wallet = tagged_from_file(&dir, DUST_WALLET_FILE)?;

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
        last_tx_id: metadata.last_tx_id,
        unshielded_utxos,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save(
    base: &Path,
    network: &str,
    seed: &WalletSeed,
    zswap_state: &ZswapLocalState<DefaultDB>,
    dust_wallet: &DustWallet<DefaultDB>,
    zswap_event_id: i64,
    dust_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: &[TrackedUtxo],
) -> Result<(), WalletError> {
    let dir = storage_dir(base, network, seed);
    std::fs::create_dir_all(&dir)
        .map_err(|e| WalletError::Storage(format!("create dir {}: {e}", dir.display())))?;

    tagged_to_file(&dir, ZSWAP_FILE, zswap_state)?;
    tagged_to_file(&dir, DUST_WALLET_FILE, dust_wallet)?;

    let metadata = StoredMetadata {
        seed_hex: hex::encode(seed.as_bytes()),
        zswap_event_id,
        dust_event_id,
        last_block_height,
        last_tx_id,
        unshielded_utxos: unshielded_utxos.iter().map(StoredUtxo::from).collect(),
    };
    let meta_path = dir.join(METADATA_FILE);
    let meta_tmp = dir.join("metadata.json.tmp");
    let meta_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| WalletError::Storage(format!("serialize metadata: {e}")))?;
    std::fs::write(&meta_tmp, &meta_json)
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", meta_tmp.display())))?;
    std::fs::rename(&meta_tmp, &meta_path)
        .map_err(|e| WalletError::Storage(format!("rename metadata: {e}")))?;

    info!(
        zswap_event_id,
        dust_event_id,
        path = %dir.display(),
        "saved wallet state to disk"
    );

    Ok(())
}
