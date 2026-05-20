use std::path::{Path, PathBuf};

use midnight_node_ledger_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_node_ledger_helpers::mn_ledger::dust::DustState;
use midnight_node_ledger_helpers::{
    BlockContext, DefaultDB, DustWallet, LedgerParameters, Timestamp, WalletSeed,
    WalletState as ZswapLocalState,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::WalletError;
use crate::state::TrackedUtxo;

const METADATA_FILE: &str = "metadata.json";
const ZSWAP_FILE: &str = "zswap.bin";
const DUST_WALLET_FILE: &str = "dust_wallet.bin";
const PARAMS_FILE: &str = "params.bin";
const BLOCK_CTX_FILE: &str = "block_ctx.bin";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMetadata {
    seed_hex: String,
    zswap_event_id: i64,
    dust_event_id: i64,
    last_block_height: i64,
    last_tx_id: Option<i64>,
    unshielded_utxos: Vec<StoredUtxo>,
    #[serde(default)]
    dust_roots: Option<StoredDustRoots>,
}

/// Compact representation of the DustState Merkle tree roots.
///
/// Persisting just the roots (a few hundred bytes) instead of the full
/// DustState (50+ MB) makes startup instant. The `well_formed()` check
/// in `StandardTrasactionInfo::prove()` only reads from `root_history`,
/// not from the actual Merkle trees.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredDustRoots {
    timestamp_secs: u64,
    commit_root_hex: String,
    gen_root_hex: String,
    commitments_first_free: u64,
    generating_tree_first_free: u64,
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

/// Lightweight dust roots loaded from disk. Historically used to reconstruct
/// a minimal `DustState` with a single `root_history` entry, sufficient for
/// the client-side `well_formed()` validation that the helpers' transaction
/// builder performs.
///
/// We no longer perform client-side validation (see [`crate::transfer`]'s
/// `build_no_validate`), so most of these fields are not currently consulted.
/// The struct is retained because old `metadata.json` files contain it and
/// keeping the schema avoids breaking backward compatibility.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CachedDustRoots {
    pub timestamp: Timestamp,
    pub commit_root_bytes: Vec<u8>,
    pub gen_root_bytes: Vec<u8>,
    pub commitments_first_free: u64,
    pub generating_tree_first_free: u64,
}

#[allow(dead_code)]
pub(crate) struct LoadedState {
    pub zswap_state: ZswapLocalState<DefaultDB>,
    pub dust_wallet: DustWallet<DefaultDB>,
    pub dust_roots: Option<CachedDustRoots>,
    pub parameters: LedgerParameters,
    pub block_context: Option<BlockContext>,
    pub seed: WalletSeed,
    pub zswap_event_id: i64,
    pub dust_event_id: i64,
    pub last_block_height: i64,
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
    let parameters = tagged_from_file(&dir, PARAMS_FILE)?;
    let block_context = if dir.join(BLOCK_CTX_FILE).exists() {
        Some(tagged_from_file(&dir, BLOCK_CTX_FILE)?)
    } else {
        None
    };

    let dust_roots = metadata.dust_roots.map(|r| {
        let commit_bytes =
            hex::decode(&r.commit_root_hex).unwrap_or_default();
        let gen_bytes =
            hex::decode(&r.gen_root_hex).unwrap_or_default();
        CachedDustRoots {
            timestamp: Timestamp::from_secs(r.timestamp_secs),
            commit_root_bytes: commit_bytes,
            gen_root_bytes: gen_bytes,
            commitments_first_free: r.commitments_first_free,
            generating_tree_first_free: r.generating_tree_first_free,
        }
    });

    let unshielded_utxos: Vec<TrackedUtxo> = metadata
        .unshielded_utxos
        .into_iter()
        .map(TrackedUtxo::try_from)
        .collect::<Result<_, _>>()?;

    info!(
        zswap_event_id = metadata.zswap_event_id,
        dust_event_id = metadata.dust_event_id,
        has_dust_roots = dust_roots.is_some(),
        unshielded_utxos = unshielded_utxos.len(),
        "loaded wallet state from disk"
    );

    Ok(Some(LoadedState {
        zswap_state,
        dust_wallet,
        dust_roots,
        parameters,
        block_context,
        seed: stored_seed,
        zswap_event_id: metadata.zswap_event_id,
        dust_event_id: metadata.dust_event_id,
        last_block_height: metadata.last_block_height,
        last_tx_id: metadata.last_tx_id,
        unshielded_utxos,
    }))
}

/// Extract the current Merkle tree roots from a DustState for compact persistence.
///
/// Returns None if the tree is empty (a `Stub` returns `Some(Fr::default())` from
/// `root()` which is zero, not a real hash). This prevents overwriting valid
/// cached roots when saving from a minimal reconstructed DustState during resume.
fn extract_dust_roots(
    dust_state: &DustState<DefaultDB>,
    timestamp: Timestamp,
) -> Option<StoredDustRoots> {
    use midnight_node_ledger_helpers::transient_crypto::merkle_tree::MerkleTreeDigest;

    let commit_root = dust_state.utxo.commitments.root()?;
    let gen_root = dust_state.generation.generating_tree.root()?;

    // Stub trees return Some(Fr::default()) from root(). Skip these to avoid
    // overwriting valid cached roots with zeros.
    let zero = MerkleTreeDigest::default();
    if commit_root == zero || gen_root == zero {
        return None;
    }

    let mut commit_buf = Vec::new();
    tagged_serialize(&commit_root, &mut commit_buf).ok()?;
    let mut gen_buf = Vec::new();
    tagged_serialize(&gen_root, &mut gen_buf).ok()?;

    Some(StoredDustRoots {
        timestamp_secs: timestamp.to_secs(),
        commit_root_hex: hex::encode(&commit_buf),
        gen_root_hex: hex::encode(&gen_buf),
        commitments_first_free: dust_state.utxo.commitments_first_free,
        generating_tree_first_free: dust_state.generation.generating_tree_first_free,
    })
}

/// Reconstruct a minimal DustState with only root_history populated.
///
/// Historically used when client-side `well_formed()` validation was active.
/// We now bypass that validation (see [`crate::transfer`]'s `build_no_validate`),
/// so this helper is currently unused. Kept for potential future use or
/// callers that opt back into client-side validation.
#[allow(dead_code)]
pub(crate) fn reconstruct_dust_state(
    roots: &CachedDustRoots,
) -> Result<DustState<DefaultDB>, WalletError> {
    use midnight_node_ledger_helpers::transient_crypto::merkle_tree::MerkleTreeDigest;

    let commit_root: MerkleTreeDigest = tagged_deserialize(&roots.commit_root_bytes[..])
        .map_err(|e| WalletError::Storage(format!("deserialize commit root: {e}")))?;
    let gen_root: MerkleTreeDigest = tagged_deserialize(&roots.gen_root_bytes[..])
        .map_err(|e| WalletError::Storage(format!("deserialize gen root: {e}")))?;

    let mut state = DustState::<DefaultDB>::default();
    state.utxo.root_history = state
        .utxo
        .root_history
        .insert(roots.timestamp, commit_root);
    state.generation.root_history = state
        .generation
        .root_history
        .insert(roots.timestamp, gen_root);
    state.utxo.commitments_first_free = roots.commitments_first_free;
    state.generation.generating_tree_first_free = roots.generating_tree_first_free;

    Ok(state)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save(
    base: &Path,
    network: &str,
    seed: &WalletSeed,
    zswap_state: &ZswapLocalState<DefaultDB>,
    dust_wallet: &DustWallet<DefaultDB>,
    dust_state: &DustState<DefaultDB>,
    parameters: &LedgerParameters,
    block_context: &Option<BlockContext>,
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
    tagged_to_file(&dir, PARAMS_FILE, parameters)?;
    if let Some(ctx) = block_context {
        tagged_to_file(&dir, BLOCK_CTX_FILE, ctx)?;
    }

    let timestamp = block_context
        .as_ref()
        .map(|bc| bc.tblock)
        .unwrap_or_else(|| Timestamp::from_secs(0));

    // Try to extract dust_roots from the in-memory state. If the state is
    // a minimal reconstruction (Stub trees from resume), extract returns None.
    // In that case, preserve the existing cached dust_roots from disk.
    let dust_roots = extract_dust_roots(dust_state, timestamp).or_else(|| {
        let meta_path = dir.join(METADATA_FILE);
        let json = std::fs::read_to_string(&meta_path).ok()?;
        let existing: StoredMetadata = serde_json::from_str(&json).ok()?;
        existing.dust_roots
    });
    let has_dust_roots = dust_roots.is_some();

    let meta_path = dir.join(METADATA_FILE);
    let meta_tmp = dir.join("metadata.json.tmp");
    let metadata = StoredMetadata {
        seed_hex: hex::encode(seed.as_bytes()),
        zswap_event_id,
        dust_event_id,
        last_block_height,
        last_tx_id,
        unshielded_utxos: unshielded_utxos.iter().map(StoredUtxo::from).collect(),
        dust_roots,
    };
    let meta_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| WalletError::Storage(format!("serialize metadata: {e}")))?;
    std::fs::write(&meta_tmp, &meta_json)
        .map_err(|e| WalletError::Storage(format!("write {}: {e}", meta_tmp.display())))?;
    std::fs::rename(&meta_tmp, &meta_path)
        .map_err(|e| WalletError::Storage(format!("rename metadata: {e}")))?;

    // Clean up old dust_global.bin if it exists (no longer needed).
    let old_dust_global = dir.join("dust_global.bin");
    if old_dust_global.exists() {
        let _ = std::fs::remove_file(&old_dust_global);
    }

    info!(
        zswap_event_id,
        dust_event_id,
        has_dust_roots,
        path = %dir.display(),
        "saved wallet state to disk"
    );

    Ok(())
}
