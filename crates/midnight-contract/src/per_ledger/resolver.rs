//! The resolver this generation's contract calls prove with.

use std::sync::LazyLock;

use super::helpers;

/// The resolver every contract call on this generation proves with: the
/// builtin Zswap and Dust keys, then whatever
/// [`register`](crate::resolver::register) made resolvable.
pub(crate) fn shared() -> &'static helpers::Resolver {
    static RESOLVER: LazyLock<helpers::Resolver> = LazyLock::new(|| {
        use helpers::{
            DUST_EXPECTED_FILES, DustResolver, FetchMode, KeyLocation, MidnightDataProvider,
            OutputMode, PUBLIC_PARAMS, ProvingKeyMaterial, Resolver,
        };

        type KeyLoaderFut = std::pin::Pin<
            Box<
                dyn std::future::Future<Output = std::io::Result<Option<ProvingKeyMaterial>>>
                    + Send
                    + Sync,
            >,
        >;
        type KeyLoader = Box<dyn Fn(KeyLocation) -> KeyLoaderFut + Send + Sync>;

        let dust = DustResolver(
            MidnightDataProvider::new(
                FetchMode::OnDemand,
                OutputMode::Log,
                DUST_EXPECTED_FILES.to_owned(),
            )
            .expect("the dust key provider takes only built-in settings"),
        );
        let external: KeyLoader = Box::new(|KeyLocation(location)| {
            Box::pin(async move {
                // A zk config may block on I/O, and the ledger polls this
                // future on its own runtime.
                tokio::task::spawn_blocking(move || {
                    crate::resolver::resolve(&location)
                        .map(|found| {
                            found.map(|a| ProvingKeyMaterial {
                                prover_key: a.prover_key,
                                verifier_key: a.verifier_key,
                                ir_source: a.zkir,
                            })
                        })
                        .map_err(std::io::Error::other)
                })
                .await
                .map_err(std::io::Error::other)?
            })
        });
        Resolver::new(PUBLIC_PARAMS.clone(), dust, external)
    });
    &RESOLVER
}
