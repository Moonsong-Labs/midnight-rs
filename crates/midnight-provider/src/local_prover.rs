//! [`LocalProver`]: the in-process prover of a ledger 9 transaction, for
//! circuits that compactc compiled for ZKIR v2 or for ZKIR v3.

use std::io::Cursor;

use async_trait::async_trait;
use midnight_helpers::ledger_9::midnight_serialize::{peek_tag, tagged_deserialize};
use midnight_helpers::ledger_9::transient_crypto::proofs::{Proof, ProvingProvider};
use midnight_helpers::ledger_9::{
    CostModel, DB, Fr, LocalProvingProvider, PUBLIC_PARAMS, ProofPreimage, ProofProvider,
    ProvenTransaction, Resolver, ResolverTrait, StdRng, UnprovenTransaction, ZswapResolver,
};

/// Proves a ledger 9 transaction in this process, for circuits of ZKIR v2
/// and ZKIR v3.
///
/// The helpers' `LocalProofServer` builds its prover from the ZKIR v2 crate
/// only, so it refuses the IR of a circuit compiled for ZKIR v3
/// (`ir-source[v3-generic]`). This prover reads the tag of each circuit's IR,
/// as the ledger's proof server does, and proves ZKIR v3 IR with
/// `midnight-zkir-v3`. Ledger 8 keeps the helpers' prover: its chain does not
/// verify a ZKIR v3 key.
pub(crate) struct LocalProver;

#[async_trait]
impl<D: DB + Clone> ProofProvider<D> for LocalProver {
    async fn prove(
        &self,
        tx: UnprovenTransaction<D>,
        rng: StdRng,
        resolver: &'static Resolver,
        cost_model: CostModel,
    ) -> ProvenTransaction<D> {
        // As in `RemoteProofServer`: a ledger 9 proof is not `Send`.
        let proving = tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime with I/O and timers")
                .block_on(async move {
                    let provider = VersionedProvingProvider(LocalProvingProvider {
                        rng,
                        resolver,
                        params: &*PUBLIC_PARAMS,
                    });
                    match tx.prove(provider, &cost_model).await {
                        Ok(proven) => proven,
                        // Unwind without the panic hook: the build reports
                        // this unwind as `WalletError::Proving`.
                        Err(err) => std::panic::resume_unwind(Box::new(err.to_string())),
                    }
                })
        });
        match proving.await {
            Ok(proven) => proven,
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        }
    }
}

/// The helpers' ZKIR v2 prover, with ZKIR v3 IR proved by `midnight-zkir-v3`.
struct VersionedProvingProvider<'a>(LocalProvingProvider<'a, StdRng, Resolver, ZswapResolver>);

/// The IR of the circuit that a preimage proves.
enum Ir {
    /// IR that the ZKIR v2 prover reads, or refuses with the tag it found.
    V2,
    V3(midnight_zkir_v3::IrSource),
}

impl VersionedProvingProvider<'_> {
    /// The IR of the circuit that `preimage` proves. The provers resolve the
    /// key again, from the resolver's cache.
    async fn ir(&self, preimage: &ProofPreimage) -> anyhow::Result<Ir> {
        let key = self
            .0
            .resolver
            .resolve_key(preimage.key_location.clone())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("no key material for circuit {}", preimage.key_location.0)
            })?;
        match peek_tag(&mut Cursor::new(&key.ir_source))?.as_str() {
            "ir-source[v3-generic]" => Ok(Ir::V3(tagged_deserialize(&mut &key.ir_source[..])?)),
            _ => Ok(Ir::V2),
        }
    }
}

impl ProvingProvider for VersionedProvingProvider<'_> {
    async fn check(&self, preimage: &ProofPreimage) -> anyhow::Result<Vec<Option<usize>>> {
        match self.ir(preimage).await? {
            Ir::V2 => self.0.check(preimage).await,
            Ir::V3(ir) => Ok(preimage.check(&ir)?),
        }
    }

    async fn prove(
        self,
        preimage: &ProofPreimage,
        overwrite_binding_input: Option<Fr>,
    ) -> anyhow::Result<Proof> {
        match self.ir(preimage).await? {
            Ir::V2 => self.0.prove(preimage, overwrite_binding_input).await,
            Ir::V3(_) => {
                let mut preimage = preimage.clone();
                if let Some(binding_input) = overwrite_binding_input {
                    preimage.binding_input = binding_input;
                }
                let (proof, _) = preimage
                    .prove::<midnight_zkir_v3::IrSource>(self.0.rng, self.0.params, self.0.resolver)
                    .await?;
                Ok(proof)
            }
        }
    }

    fn split(&mut self) -> Self {
        Self(self.0.split())
    }

    fn resolver(&self) -> &impl ResolverTrait {
        self.0.resolver
    }
}
