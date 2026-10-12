//! The [`Network`] identifier and its conversions from strings.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

/// A typed Midnight network identifier.
///
/// The name is the suffix of the bech32 HRP of a wallet address
/// (`mn_addr_<network>1...`), and the ledger state's `network_id` holds the
/// same name. Mainnet has no suffix (`mn_addr1...`), as in the upstream
/// `IntoWalletAddress` convention.
///
/// Every SDK item that takes a network, such as `WalletBuilder::with_network`
/// in `midnight-wallet` and the [`Seed`](crate::hd::Seed) address methods,
/// takes `impl Into<Network>`. The typed form and the string forms work:
///
/// ```rust
/// use midnight_types::Network;
/// use midnight_types::hd::Seed;
/// # let seed = Seed::from_bytes(&[1u8; 32]).unwrap();
///
/// seed.shielded_address(Network::Preprod);
/// seed.shielded_address("preprod");
/// seed.shielded_address(String::from("preprod"));
/// ```
///
/// The variants cover the networks the SDK knows by name. Any other name,
/// such as the name of a custom devnet, goes through [`Network::Other`]
/// without loss.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Network {
    /// The local devnet. Its genesis funds the seed `0000…0001` with test
    /// tokens.
    Undeployed,
    /// Public pre-production testnet (faucet-funded).
    Preprod,
    /// Public testnet.
    Testnet,
    /// Mainnet. Its bech32 addresses have **no** `_<name>` suffix, as upstream
    /// `IntoWalletAddress::network_suffix` gives.
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
            assert_eq!(AsRef::<str>::as_ref(&got), name);
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
}
