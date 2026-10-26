//! Proving keys for contract circuits, found by the hash of their verifier key.
//!
//! The ledger helpers hold the resolver a build proves with as a `&'static`
//! borrow, so one resolver per ledger generation serves every contract this
//! process calls (see the generation modules' `resolver`). It finds a
//! circuit's keys through the registry here, which both share: a call registers the zk config
//! it proves with under the SHA-256 of the circuit's verifier key, and names
//! that hash in the key location it hands the prover. The location has the
//! `contract:<address>/<circuit>?vk=<hash>` shape the upstream helpers and
//! compact-js resolve.
//!
//! The registry holds each zk config weakly, so a registration lives exactly
//! as long as the contract handle that made it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use sha2::{Digest, Sha256};

use crate::error::ContractError;
use crate::zk_config::{ZkArtifacts, ZkConfigError, ZkConfigProvider};

type Registry = HashMap<[u8; 32], Vec<Weak<dyn ZkConfigProvider>>>;

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

/// Register `zk_config` as the source of `circuit`'s keys, and return the key
/// location that resolves to them.
pub(crate) fn register(
    address_hex: &str,
    circuit: &str,
    zk_config: &Arc<dyn ZkConfigProvider>,
) -> Result<String, ContractError> {
    let verifier_key = zk_config
        .verifier_key(circuit)
        .map_err(|e| ContractError::Construction(format!("verifier key {circuit}: {e}")))?;
    let hash: [u8; 32] = Sha256::digest(&verifier_key).into();
    let mut registry = REGISTRY.lock().expect("resolver registry poisoned");
    registry.retain(|_, configs| {
        configs.retain(|config| config.strong_count() > 0);
        !configs.is_empty()
    });
    let configs = registry.entry(hash).or_default();
    let zk_config = Arc::downgrade(zk_config);
    if !configs.iter().any(|config| config.ptr_eq(&zk_config)) {
        configs.push(zk_config);
    }
    Ok(format!(
        "contract:{address_hex}/{circuit}?vk={}",
        hex::encode(hash)
    ))
}

/// The artifacts `location` names, or `None` when it names no live
/// registration. A location in any other shape is not a contract key, which
/// is also `None`.
pub(crate) fn resolve(location: &str) -> Result<Option<ZkArtifacts>, ZkConfigError> {
    let Some((circuit, hash)) = parse(location) else {
        return Ok(None);
    };
    let zk_config = REGISTRY
        .lock()
        .expect("resolver registry poisoned")
        .get(&hash)
        .and_then(|configs| configs.iter().find_map(Weak::upgrade));
    let Some(zk_config) = zk_config else {
        return Ok(None);
    };
    let artifacts = zk_config.artifacts(circuit)?;
    if <[u8; 32]>::from(Sha256::digest(&artifacts.verifier_key)) != hash {
        return Err(ZkConfigError::Backend(format!(
            "the verifier key for circuit `{circuit}` changed after the call registered it"
        )));
    }
    Ok(Some(artifacts))
}

/// Split `contract:<address>/<circuit>?vk=<hash>` into the circuit and the
/// verifier-key hash.
fn parse(location: &str) -> Option<(&str, [u8; 32])> {
    let rest = location.strip_prefix("contract:")?;
    let (_address, rest) = rest.split_once('/')?;
    let (circuit, hash) = rest.split_once("?vk=")?;
    let hash: [u8; 32] = hex::decode(hash).ok()?.try_into().ok()?;
    (!circuit.is_empty()).then_some((circuit, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OneCircuit(ZkArtifacts);

    impl ZkConfigProvider for OneCircuit {
        fn prover_key(&self, _: &str) -> Result<Vec<u8>, ZkConfigError> {
            Ok(self.0.prover_key.clone())
        }
        fn verifier_key(&self, _: &str) -> Result<Vec<u8>, ZkConfigError> {
            Ok(self.0.verifier_key.clone())
        }
        fn zkir(&self, _: &str) -> Result<Vec<u8>, ZkConfigError> {
            Ok(self.0.zkir.clone())
        }
    }

    fn config(verifier_key: &[u8]) -> Arc<dyn ZkConfigProvider> {
        Arc::new(OneCircuit(ZkArtifacts {
            prover_key: b"prover".to_vec(),
            verifier_key: verifier_key.to_vec(),
            zkir: b"zkir".to_vec(),
        }))
    }

    /// Two contracts that both name a circuit `increment` must each prove
    /// with their own keys, which a registry keyed by circuit name would mix
    /// up.
    #[test]
    fn a_location_resolves_to_the_keys_of_the_contract_that_registered_it() {
        let first = config(b"first verifier key");
        let second = config(b"second verifier key");
        let at_first = register("aa", "increment", &first).unwrap();
        let at_second = register("bb", "increment", &second).unwrap();

        let found = resolve(&at_first).unwrap().expect("registered");
        assert_eq!(found.verifier_key, b"first verifier key");
        let found = resolve(&at_second).unwrap().expect("registered");
        assert_eq!(found.verifier_key, b"second verifier key");
    }

    /// The registry must not keep a dropped contract's artifacts alive.
    #[test]
    fn a_dropped_zk_config_stops_resolving() {
        let config = config(b"short-lived verifier key");
        let location = register("cc", "increment", &config).unwrap();
        drop(config);
        assert!(resolve(&location).unwrap().is_none());
    }

    /// Two handles of one compiled contract register two zk configs under
    /// one verifier key. Dropping one must not stop the other from proving.
    #[test]
    fn a_location_resolves_while_any_zk_config_under_it_lives() {
        let kept = config(b"shared verifier key");
        let dropped = config(b"shared verifier key");
        let location = register("dd", "increment", &kept).unwrap();
        register("dd", "increment", &dropped).unwrap();
        drop(dropped);

        let found = resolve(&location).unwrap().expect("still registered");
        assert_eq!(found.verifier_key, b"shared verifier key");
    }

    /// A handle registers its zk config on every call, so the registry must
    /// not grow with the number of calls.
    #[test]
    fn registering_one_zk_config_again_adds_no_entry() {
        let config = config(b"re-registered verifier key");
        register("ee", "increment", &config).unwrap();
        let location = register("ee", "increment", &config).unwrap();

        let (_, hash) = parse(&location).unwrap();
        let registry = REGISTRY.lock().unwrap();
        assert_eq!(registry[&hash].len(), 1);
    }
}
