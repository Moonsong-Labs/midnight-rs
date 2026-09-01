//! One API over the ledger generations this build carries.

use async_trait::async_trait;

use crate::{Error, Generation, Health, Landed, Opening, OpeningField, Verdict};

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

    /// Combine transactions into one. Bytes in, bytes out, so no conversion.
    async fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, Error>;

    /// Fund a transaction from the attached wallet. Bytes in, bytes out.
    async fn balance_transaction(&self, tx: &[u8]) -> Result<Vec<u8>, Error>;

    /// A contract's state, hex-encoded as the indexer serves it.
    ///
    /// The encoding is the ledger's own tagged form, and `StateValue` carries
    /// the same tag in both generations, so this crosses the boundary without
    /// conversion.
    async fn contract_state(&self, address: &str) -> Result<Option<String>, Error>;

    /// Deploy a contract from a compiled-artifact directory, and return its
    /// address.
    async fn deploy(&self, zk_config_dir: &str, opening: Opening) -> Result<String, Error>;

    /// Submit a transaction and wait for it to be finalized.
    ///
    /// Waiting is part of this call because the handle a submission returns is
    /// the generation's own, holding a node subscription that cannot cross
    /// this boundary.
    async fn submit_and_wait(&self, tx: &[u8]) -> Result<Landed, Error>;
}

/// Implement [`Backend`] over one generation's provider.
///
/// Each generation's `MidnightProvider` is a distinct type with the same shape,
/// so the bodies are identical and only the crate differs. Writing them out
/// per generation is what lets the conversion to the neutral types happen at
/// this boundary and nowhere else.
macro_rules! backend_over {
    ($module:ident, $contract:ident, $generation:expr) => {
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

            async fn deploy(&self, zk_config_dir: &str, opening: Opening) -> Result<String, Error> {
                let fields = opening
                    .fields
                    .into_iter()
                    .map(|field| match field {
                        OpeningField::Cell(value) => $contract::InitialField::Cell(value),
                        OpeningField::Counter(value) => $contract::InitialField::Counter(value),
                        OpeningField::Map => $contract::InitialField::Map,
                        OpeningField::List => $contract::InitialField::List,
                        OpeningField::MerkleTree => $contract::InitialField::MerkleTree,
                    })
                    .collect();
                let contract = $contract::Contract::deploy(self)
                    .with_initial_state($contract::InitialState::new(fields))
                    .with_zk_config(zk_config_dir)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(contract.address().to_owned())
            }

            async fn contract_state(&self, address: &str) -> Result<Option<String>, Error> {
                $module::Provider::get_contract_state(self, address, None)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
                // Sync on the provider: merging is local, with no chain call.
                self.merge_transactions(txs)
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn balance_transaction(&self, tx: &[u8]) -> Result<Vec<u8>, Error> {
                self.balance_transaction(tx)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))
            }

            async fn submit_and_wait(&self, tx: &[u8]) -> Result<Landed, Error> {
                let pending = self
                    .submit(tx)
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                let (landed, _) = pending
                    .wait_finalized()
                    .await
                    .map_err(|e| Error::Chain(e.to_string()))?;
                Ok(Landed {
                    block_hash: landed.block_hash,
                    extrinsic_hash: landed.extrinsic_hash,
                    transaction_hash: *landed.transaction_hash.as_bytes(),
                    verdict: match landed.verdict {
                        $module::Verdict::Success => Verdict::Success,
                        $module::Verdict::PartialSuccess => Verdict::PartialSuccess,
                        $module::Verdict::Failure => Verdict::Failure,
                    },
                })
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

backend_over!(p8, c8, Generation::Ledger8);
backend_over!(p9, c9, Generation::Ledger9);

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

    /// The neutral surface is the same on both generations, so a caller can
    /// hold either behind one pointer. This is the property dispatch rests on:
    /// were the two providers not interchangeable here, `Client` could not
    /// choose between them at runtime.
    #[test]
    fn both_generations_satisfy_one_trait_object() {
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(
                p8::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1")
                    .expect("construct"),
            ),
            Box::new(
                p9::MidnightProvider::new("ws://127.0.0.1:1", "http://127.0.0.1:1")
                    .expect("construct"),
            ),
        ];
        let seen: Vec<Generation> = backends.iter().map(|b| b.generation()).collect();
        assert_eq!(seen, vec![Generation::Ledger8, Generation::Ledger9]);
    }
}
