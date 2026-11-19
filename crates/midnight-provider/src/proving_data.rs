//! [`fetch_proving_data`]: the files of the local prover, fetched ahead of a
//! proof.

use std::io;

use midnight_helpers::{
    DUST_EXPECTED_FILES, FetchMode, MidnightDataProvider, OutputMode, ZSWAP_EXPECTED_FILES,
};

/// The size, as `k`, of the largest built-in circuit: the Zswap spend circuit
/// of ledger 9, as its verifier key states.
const BUILTIN_MAX_K: u8 = 15;

/// Fetches the files that [`Prover::local`](crate::Prover::local) reads and
/// that no contract carries, so that a later proof needs no network.
///
/// The files are the built-in Zswap and Dust keys of ledger 9, and the
/// universal parameters `bls_midnight_2p<k>` for each `k` from 0 to `max_k`.
/// Give as `max_k` the size of the largest contract circuit that the app
/// proves, as the `k` of its `2^k` rows. For a smaller `max_k`, the function
/// fetches the parameters up to `k = 15`, because the Zswap spend circuit has
/// `2^15` rows. So `max_k` matters only for a contract circuit of more than
/// `2^15` rows.
///
/// The function fills the cache that [`Prover::local`](crate::Prover::local)
/// reads, from the same source. Set `MIDNIGHT_PP` at the start of the process:
/// the local prover reads it only once. A file that the cache holds with its
/// expected SHA-256 is not downloaded again. A download goes into the cache
/// only when its SHA-256 is the expected one.
///
/// # Errors
///
/// The [`io::Error`] of the upstream data provider:
///
/// - [`NotFound`](io::ErrorKind::NotFound) when none of `MIDNIGHT_PP`,
///   `XDG_CACHE_HOME` and `HOME` is set.
/// - [`InvalidInput`](io::ErrorKind::InvalidInput) when upstream knows no
///   parameters for `max_k`. The function then downloads nothing.
/// - [`InvalidData`](io::ErrorKind::InvalidData) when a download fails three
///   times, or when the cache holds a file whose SHA-256 is not the expected
///   one. The function does not replace that file: delete it and call again.
/// - Any other I/O error.
///
/// # Panics
///
/// When `MIDNIGHT_PARAM_SOURCE` is set to a value that is not a URL.
///
/// ```rust,no_run
/// # async fn run() -> std::io::Result<()> {
/// // The largest circuit of the app's contracts has 2^16 rows.
/// midnight_provider::fetch_proving_data(16).await?;
/// # Ok(())
/// # }
/// ```
pub async fn fetch_proving_data(max_k: u8) -> io::Result<()> {
    let data = MidnightDataProvider::new(FetchMode::OnDemand, OutputMode::Log, builtin_keys())?;
    for name in file_names(max_k) {
        data.fetch(&name).await?;
    }
    Ok(())
}

/// The built-in key files of ledger 9, each with its SHA-256.
fn builtin_keys() -> Vec<(&'static str, [u8; 32], &'static str)> {
    [ZSWAP_EXPECTED_FILES, DUST_EXPECTED_FILES].concat()
}

/// The names of the files that [`fetch_proving_data`] fetches for `max_k`.
fn file_names(max_k: u8) -> Vec<String> {
    // Parameters go largest first and before the keys, so that a `k` that
    // upstream does not know fails before any download.
    (0..=max_k.max(BUILTIN_MAX_K))
        .rev()
        .map(MidnightDataProvider::name_k)
        .chain(builtin_keys().into_iter().map(|(name, ..)| name.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hold_the_built_in_keys_and_each_size_through_the_largest() {
        let names = file_names(10);
        for (key, ..) in ZSWAP_EXPECTED_FILES.iter().chain(DUST_EXPECTED_FILES) {
            assert!(names.iter().any(|name| name == key), "{key} is missing");
        }
        assert!(
            names.contains(&MidnightDataProvider::name_k(BUILTIN_MAX_K)),
            "a small max_k drops the parameters of the built-in circuits"
        );
        assert!(file_names(20).contains(&MidnightDataProvider::name_k(20)));
    }
}
