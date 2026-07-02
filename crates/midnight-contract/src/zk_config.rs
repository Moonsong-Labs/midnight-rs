//! Pluggable source of a contract's compiled ZK artifacts (prover key, verifier
//! key, ZKIR), per circuit. The SDK consumes the bytes; where they come from —
//! filesystem, embedded bundle, memory, a remote service — is the implementor's
//! concern. This mirrors midnight-js's `ZKConfigProvider`.
//!
//! The trait is synchronous: the ledger's `ExternalResolver` requires a
//! `Send + Sync` key-loading future, which an `async fn` trait method (via
//! `async-trait`) does not produce. The call path wraps provider lookups in
//! `spawn_blocking`, so a blocking implementation never stalls the runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A contract's compiled ZK artifacts for one circuit, as raw `compactc` output
/// bytes. Field order matches midnight-ledger's `ProvingKeyMaterial`.
#[derive(Clone)]
pub struct ZkArtifacts {
    pub prover_key: Vec<u8>,
    pub verifier_key: Vec<u8>,
    pub zkir: Vec<u8>,
}

/// Errors a [`ZkConfigProvider`] may return.
#[derive(Debug, thiserror::Error)]
pub enum ZkConfigError {
    /// The provider does not serve `circuit`. Callers that probe many circuits
    /// (e.g. the proving resolver) treat this as "not mine", not a failure.
    #[error("no zk artifacts for circuit `{0}`")]
    NotFound(String),
    /// Some but not all of a circuit's artifacts are present — a misconfigured
    /// source, distinct from a circuit that is simply absent.
    #[error("incomplete zk artifacts for circuit `{circuit}`: {detail}")]
    Incomplete { circuit: String, detail: String },
    /// The backing store failed while reading `circuit`'s artifacts.
    #[error("reading zk artifacts for `{circuit}`: {source}")]
    Io {
        circuit: String,
        #[source]
        source: std::io::Error,
    },
    /// Any other backend-specific failure.
    #[error("{0}")]
    Backend(String),
}

/// Source of a contract's compiled ZK artifacts, per circuit id.
///
/// The default [`FsZkConfigProvider`] reads them from a compiled contract
/// directory; implement this trait to serve them from anywhere (an embedded
/// bundle, memory, a service). Used by the deploy path (verifier keys, via
/// [`list_circuits`](Self::list_circuits)) and the call/prove path (all three,
/// via [`artifacts`](Self::artifacts)).
pub trait ZkConfigProvider: Send + Sync {
    /// Prover key bytes for `circuit`; [`ZkConfigError::NotFound`] if absent.
    fn prover_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError>;
    /// Verifier key bytes for `circuit`; [`ZkConfigError::NotFound`] if absent.
    fn verifier_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError>;
    /// ZKIR bytes for `circuit`; [`ZkConfigError::NotFound`] if absent.
    fn zkir(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError>;

    /// All three artifacts for `circuit`. The default fetches each in turn and
    /// surfaces [`ZkConfigError::NotFound`] when the circuit is absent, so the
    /// proving resolver can map "not found" to "not my circuit". Implementations
    /// that can distinguish a partial set should override this to return
    /// [`ZkConfigError::Incomplete`] (see [`FsZkConfigProvider`]).
    fn artifacts(&self, circuit: &str) -> Result<ZkArtifacts, ZkConfigError> {
        Ok(ZkArtifacts {
            prover_key: self.prover_key(circuit)?,
            verifier_key: self.verifier_key(circuit)?,
            zkir: self.zkir(circuit)?,
        })
    }

    /// The circuit ids this provider can enumerate, or `None` if it cannot. The
    /// deploy path needs this to populate a verifier key per circuit into
    /// contract state; a provider returning `None` can drive calls but not a
    /// deploy.
    fn list_circuits(&self) -> Result<Option<Vec<String>>, ZkConfigError> {
        Ok(None)
    }
}

/// Reads a contract's ZK artifacts from a compiled contract directory:
/// `{base}/keys/{circuit}.prover`, `{base}/keys/{circuit}.verifier`, and
/// `{base}/zkir/{circuit}.bzkir`. `base` may be the directory containing `keys/`
/// and `zkir/`, or those subdirectories' parent — both are accepted.
pub struct FsZkConfigProvider {
    base: PathBuf,
}

impl FsZkConfigProvider {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Resolve the directory that contains `keys/` and `zkir/`.
    fn base_dir(&self) -> PathBuf {
        if self.base.join("keys").is_dir() {
            self.base.clone()
        } else {
            self.base
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.base.clone())
        }
    }

    fn read_opt(
        &self,
        sub: &str,
        circuit: &str,
        ext: &str,
    ) -> Result<Option<Vec<u8>>, ZkConfigError> {
        let path = self.base_dir().join(sub).join(format!("{circuit}.{ext}"));
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ZkConfigError::Io {
                circuit: circuit.to_string(),
                source,
            }),
        }
    }

    fn require(&self, sub: &str, circuit: &str, ext: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.read_opt(sub, circuit, ext)?
            .ok_or_else(|| ZkConfigError::NotFound(circuit.to_string()))
    }
}

impl ZkConfigProvider for FsZkConfigProvider {
    fn prover_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.require("keys", circuit, "prover")
    }

    fn verifier_key(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.require("keys", circuit, "verifier")
    }

    fn zkir(&self, circuit: &str) -> Result<Vec<u8>, ZkConfigError> {
        self.require("zkir", circuit, "bzkir")
    }

    fn artifacts(&self, circuit: &str) -> Result<ZkArtifacts, ZkConfigError> {
        // All-or-nothing: every artifact absent means "not this contract's
        // circuit" (NotFound); a partial set is a broken install (Incomplete).
        let prover_key = self.read_opt("keys", circuit, "prover")?;
        let verifier_key = self.read_opt("keys", circuit, "verifier")?;
        let zkir = self.read_opt("zkir", circuit, "bzkir")?;
        match (prover_key, verifier_key, zkir) {
            (None, None, None) => Err(ZkConfigError::NotFound(circuit.to_string())),
            (Some(prover_key), Some(verifier_key), Some(zkir)) => Ok(ZkArtifacts {
                prover_key,
                verifier_key,
                zkir,
            }),
            (prover_key, verifier_key, zkir) => {
                let missing: Vec<&str> = [
                    ("prover", prover_key.is_some()),
                    ("verifier", verifier_key.is_some()),
                    ("bzkir", zkir.is_some()),
                ]
                .into_iter()
                .filter_map(|(name, present)| (!present).then_some(name))
                .collect();
                Err(ZkConfigError::Incomplete {
                    circuit: circuit.to_string(),
                    detail: format!("missing [{}]", missing.join(", ")),
                })
            }
        }
    }

    fn list_circuits(&self) -> Result<Option<Vec<String>>, ZkConfigError> {
        let keys_dir = self.base_dir().join("keys");
        let entries = match std::fs::read_dir(&keys_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
            Err(source) => {
                return Err(ZkConfigError::Io {
                    circuit: String::new(),
                    source,
                });
            }
        };
        let mut circuits = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|source| ZkConfigError::Io {
                    circuit: String::new(),
                    source,
                })?
                .path();
            if path.extension().and_then(|e| e.to_str()) == Some("verifier") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    circuits.push(stem.to_string());
                }
            }
        }
        Ok(Some(circuits))
    }
}

/// Converts a value into a [`ZkConfigProvider`] for the builder setters.
///
/// A path (`&str`, `String`, `&Path`, `&PathBuf`, `PathBuf`) becomes a
/// [`FsZkConfigProvider`]; an `Arc<P>` of any provider passes through. This lets
/// `with_zk_config` accept either a compiled-contract directory or a custom
/// provider, mirroring the crate's `IntoAddress` conversion.
pub trait IntoZkConfig {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider>;
}

impl<P: ZkConfigProvider + 'static> IntoZkConfig for Arc<P> {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        self
    }
}

impl IntoZkConfig for Arc<dyn ZkConfigProvider> {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        self
    }
}

impl IntoZkConfig for &str {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}

impl IntoZkConfig for String {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}

impl IntoZkConfig for &String {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}

impl IntoZkConfig for &Path {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}

impl IntoZkConfig for &PathBuf {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}

impl IntoZkConfig for PathBuf {
    fn into_zk_config(self) -> Arc<dyn ZkConfigProvider> {
        Arc::new(FsZkConfigProvider::new(self))
    }
}
