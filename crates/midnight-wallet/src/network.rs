//! Typed [`Network`] identifier — replaces stringly-typed `&str` network names
//! across the SDK (sync, address derivation, etc.).
//!
//! Used as the bech32 HRP suffix for Midnight wallet addresses
//! (`mn_addr_<network>1...`) and matched against ledger state's
//! `network_id`. Mainnet has no suffix — `mn_addr1...` — to match the upstream
//! [`midnight_helpers::IntoWalletAddress`] convention.
//!
//! All SDK entry points that accept a network (sync, address derivation, etc.)
//! take `impl Into<Network>`. Both typed and string forms work:
//!
//! ```rust,ignore
//! use midnight_wallet::Network;
//!
//! address::derive_shielded(&seed, Network::Preprod);
//! address::derive_shielded(&seed, "preprod");                  // From<&str>
//! address::derive_shielded(&seed, env::var("NETWORK").unwrap());  // From<String>
//! ```
//!
//! Unknown names round-trip through [`Network::Other`] so you can still target
//! a custom devnet that the SDK doesn't have a named variant for.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

/// A Midnight network identifier.
///
/// Maps to the bech32 HRP suffix on wallet addresses. The variants cover the
/// networks the SDK knows by name; any other named network round-trips through
/// [`Network::Other`] without loss.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Network {
    /// Local dev devnet — the genesis seed `0000…0001` is funded with test
    /// tokens at genesis.
    Undeployed,
    /// Public pre-production testnet (faucet-funded).
    Preprod,
    /// Public testnet.
    Testnet,
    /// Mainnet — bech32 addresses have **no** `_<name>` suffix, matching
    /// upstream `IntoWalletAddress::network_suffix`.
    Mainnet,
    /// A custom or future network. The contained string is the literal name
    /// that appears in the bech32 HRP.
    Other(String),
}

impl Network {
    /// The literal network name as used in the bech32 HRP (and matched against
    /// the ledger's `network_id`).
    pub fn as_str(&self) -> &str {
        match self {
            Network::Undeployed => "undeployed",
            Network::Preprod => "preprod",
            Network::Testnet => "testnet",
            Network::Mainnet => "mainnet",
            Network::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Network {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for Network {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "undeployed" => Network::Undeployed,
            "preprod" => Network::Preprod,
            "testnet" => Network::Testnet,
            "mainnet" => Network::Mainnet,
            other => Network::Other(other.to_string()),
        })
    }
}

impl From<&str> for Network {
    fn from(s: &str) -> Self {
        // FromStr is infallible.
        s.parse().unwrap()
    }
}

impl From<String> for Network {
    fn from(s: String) -> Self {
        // Normalize via the str path so e.g. `"preprod".to_string().into()`
        // round-trips to `Network::Preprod`, not `Network::Other("preprod")`.
        s.as_str().into()
    }
}

impl From<&String> for Network {
    fn from(s: &String) -> Self {
        s.as_str().into()
    }
}

impl From<&Network> for Network {
    fn from(n: &Network) -> Self {
        n.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_round_trip() {
        for (name, want) in [
            ("undeployed", Network::Undeployed),
            ("preprod", Network::Preprod),
            ("testnet", Network::Testnet),
            ("mainnet", Network::Mainnet),
        ] {
            let got: Network = name.into();
            assert_eq!(got, want);
            assert_eq!(got.as_str(), name);
            assert_eq!(format!("{got}"), name);
        }
    }

    #[test]
    fn unknown_name_lands_in_other() {
        let n: Network = "custom-devnet".into();
        assert_eq!(n, Network::Other("custom-devnet".into()));
        assert_eq!(n.as_str(), "custom-devnet");
    }

    #[test]
    fn from_string_normalizes_known_names() {
        let s: String = "preprod".into();
        let n: Network = s.into();
        assert_eq!(n, Network::Preprod);
    }

    #[test]
    fn as_ref_str_matches_as_str() {
        let n = Network::Preprod;
        assert_eq!(AsRef::<str>::as_ref(&n), n.as_str());
    }
}
