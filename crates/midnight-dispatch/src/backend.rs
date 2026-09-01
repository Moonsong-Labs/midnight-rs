//! One API over the ledger generations this build carries.

use async_trait::async_trait;

use crate::{Error, Generation, Health};

/// What a client needs from a chain, without naming a ledger generation.
///
/// A trait object rather than an enum: a later generation is one more
/// implementation, not an edit to every method here.
#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// The generation this backend speaks.
    fn generation(&self) -> Generation;

    /// The ledger version the node reports.
    async fn ledger_version(&self) -> Result<String, Error>;

    /// Node and indexer reachability.
    async fn health(&self) -> Result<Health, Error>;
}

/// Implement [`Backend`] over one generation's provider.
///
/// Each generation's `MidnightProvider` is a distinct type with the same shape,
/// so the bodies are identical and only the crate differs. Writing them out
/// per generation is what lets the conversion to the neutral types happen at
/// this boundary and nowhere else.
macro_rules! backend_over {
    ($module:ident, $generation:expr) => {
        #[async_trait]
        impl Backend for $module::MidnightProvider {
            fn generation(&self) -> Generation {
                $generation
            }

            async fn ledger_version(&self) -> Result<String, Error> {
                self.ledger_version()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn health(&self) -> Result<Health, Error> {
                let health = self
                    .health()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Health {
                    node_connected: health.node_connected,
                    indexer_connected: health.indexer_connected,
                    block_height: health.block_height,
                    peers: health.peers,
                    is_syncing: health.is_syncing,
                })
            }
        }
    };
}

backend_over!(p8, Generation::Ledger8);
backend_over!(p9, Generation::Ledger9);

#[cfg(test)]
mod tests {
    use super::*;

    /// Each generation's provider reports its own generation, so a connection
    /// dispatches to the ledger the chain runs rather than a compiled-in one.
    /// Both are linked here: were they one crate, this could not compile.
    #[test]
    fn each_backend_reports_its_own_generation() {
        let eight =
            p8::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").expect("construct");
        let nine =
            p9::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1").expect("construct");
        assert_eq!(Backend::generation(&eight), Generation::Ledger8);
        assert_eq!(Backend::generation(&nine), Generation::Ledger9);
    }
}
