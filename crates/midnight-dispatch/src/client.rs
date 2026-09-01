//! A client that reads the chain's ledger generation and speaks it.

use crate::backend::Backend;
use crate::{Error, Generation, Health, Landed, Opening, generation_of};

/// A connection to a Midnight chain, on whichever ledger generation it runs.
pub struct Client {
    backend: Box<dyn Backend>,
}

impl Client {
    /// Connect, ask the node which ledger it runs, and speak that one.
    ///
    /// Returns [`Error::Generation`] when the chain runs a generation this
    /// build does not carry, which names the ledger so an operator can tell an
    /// old SDK from an unreachable node.
    pub async fn connect(node_url: &str, indexer_url: &str) -> Result<Self, Error> {
        // The version query is the same RPC on every generation, so any
        // backend can ask it before the generation is known.
        let probe = p8::MidnightProvider::new(node_url, indexer_url)
            .map_err(|e| Error::Chain(e.to_string()))?;
        let reported = Backend::ledger_version(&probe).await?;

        let backend: Box<dyn Backend> = match generation_of(&reported)? {
            Generation::Ledger8 => Box::new(probe),
            Generation::Ledger9 => Box::new(
                p9::MidnightProvider::new(node_url, indexer_url)
                    .map_err(|e| Error::Chain(e.to_string()))?,
            ),
        };
        Ok(Self { backend })
    }

    /// The generation the connected chain runs.
    pub fn generation(&self) -> Generation {
        self.backend.generation()
    }

    /// The ledger version the node reports.
    pub async fn ledger_version(&self) -> Result<String, Error> {
        self.backend.ledger_version().await
    }

    /// Node and indexer reachability.
    pub async fn health(&self) -> Result<Health, Error> {
        self.backend.health().await
    }

    /// A contract's state, hex-encoded, from whichever generation the chain
    /// runs. Returns `None` when the indexer does not know the contract.
    pub async fn contract_state(&self, address: &str) -> Result<Option<String>, Error> {
        self.backend.contract_state(address).await
    }

    /// Deploy a contract from a compiled-artifact directory.
    ///
    /// Returns the address it deployed to. The generation is whichever the
    /// connected chain runs.
    pub async fn deploy(&self, zk_config_dir: &str, opening: Opening) -> Result<String, Error> {
        self.backend.deploy(zk_config_dir, opening).await
    }

    /// Combine transactions into one.
    pub async fn merge_transactions(&self, txs: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
        self.backend.merge_transactions(txs).await
    }

    /// Fund a transaction from the attached wallet.
    pub async fn balance_transaction(&self, tx: &[u8]) -> Result<Vec<u8>, Error> {
        self.backend.balance_transaction(tx).await
    }

    /// Submit a transaction and wait for the chain to finalize it.
    pub async fn submit_and_wait(&self, tx: &[u8]) -> Result<Landed, Error> {
        self.backend.submit_and_wait(tx).await
    }
}
