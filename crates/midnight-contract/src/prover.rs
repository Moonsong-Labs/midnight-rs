use std::path::{Path, PathBuf};

/// Configures how zero-knowledge proofs are generated for transactions.
///
/// Both local and remote proving require a `keys_dir` containing the compiled
/// contract's proving artifacts (`keys/` and `zkir/` subdirectories). For local
/// proving, the keys are used directly. For remote proving, the keys are
/// serialized into the HTTP request body sent to the proof server.
#[derive(Debug, Clone)]
pub enum Prover {
    /// Prove locally using the CPU. Slower but requires no external services.
    Local { keys_dir: PathBuf },
    /// Delegate proving to an HTTP proof server (e.g., midnightntwrk/proof-server).
    /// Faster for large circuits, supports GPU acceleration.
    Remote { url: String, keys_dir: PathBuf },
}

impl Prover {
    /// Create a local prover with the given keys directory.
    pub fn local(keys_dir: impl Into<PathBuf>) -> Self {
        Prover::Local {
            keys_dir: keys_dir.into(),
        }
    }

    /// Create a remote prover pointing to an HTTP proof server.
    pub fn remote(url: impl Into<String>, keys_dir: impl Into<PathBuf>) -> Self {
        Prover::Remote {
            url: url.into(),
            keys_dir: keys_dir.into(),
        }
    }

    /// The keys directory for this prover.
    pub fn keys_dir(&self) -> &Path {
        match self {
            Prover::Local { keys_dir } | Prover::Remote { keys_dir, .. } => keys_dir,
        }
    }
}
