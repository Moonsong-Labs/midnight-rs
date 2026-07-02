/// Configures how zero-knowledge proofs are generated for transactions.
///
/// The default is `Local` (CPU-based proving). Use `Remote` to delegate
/// proving to an HTTP proof server (e.g., midnightntwrk/proof-server).
///
/// Both variants require ZK keys to be configured separately via
/// `.with_zk_config(...)` on the builder or on the contract.
#[derive(Debug, Clone, Default)]
pub enum Prover {
    /// Prove locally using the CPU. Default.
    #[default]
    Local,
    /// Delegate proving to an HTTP proof server.
    Remote(String),
}
