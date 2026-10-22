//! The [`Network`] identifier and the parse of a network name.

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
/// takes `impl Into<Network>`, so a `Network` and a `&Network` both work.
/// Parse a name, such as one from the configuration of a program, with
/// [`str::parse`]. The parse refuses a name that is not safe in an address
/// or in a storage path:
///
/// ```rust
/// use midnight_types::Network;
/// use midnight_types::hd::Seed;
/// # let seed = Seed::from_bytes(&[1u8; 32]).unwrap();
///
/// let network: Network = "preprod".parse()?;
/// assert_eq!(network, Network::Preprod);
/// seed.shielded_address(&network);
///
/// let custom: Network = "my-devnet".parse()?;
/// assert_eq!(custom.as_str(), "my-devnet");
///
/// assert!("Preprod".parse::<Network>().is_err());
/// # Ok::<(), midnight_types::InvalidNetworkName>(())
/// ```
///
/// The variants cover the networks the SDK knows by name. Any other valid
/// name, such as the name of a custom devnet, goes through [`Network::Other`]
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
    /// A custom or future network. The name is the literal name that appears
    /// in the bech32 HRP.
    Other(NetworkName),
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
            Network::Other(name) => name.as_str(),
        }
    }
}

const MAX_NAME_LEN: usize = 83 - "mn_shield-addr_".len();

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
    type Err = InvalidNetworkName;

    /// Parse a network name.
    ///
    /// `undeployed`, `preprod`, `testnet` and `mainnet` give their variants,
    /// and any other valid name gives [`Network::Other`]. A valid name has 1
    /// to 68 characters, each a lowercase ASCII letter, a digit, `-` or `_`.
    /// Such a name is safe in the HRP of an address, where upstream
    /// derivation panics on an invalid HRP. It is also safe as a component of
    /// a wallet's storage path. The bound keeps the longest HRP,
    /// `mn_shield-addr_<name>`, within the bech32 limit of 83 characters.
    ///
    /// # Errors
    ///
    /// [`InvalidNetworkName`] for a name that breaks this rule.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(match name {
            "undeployed" => Network::Undeployed,
            "preprod" => Network::Preprod,
            "testnet" => Network::Testnet,
            "mainnet" => Network::Mainnet,
            _ if (1..=MAX_NAME_LEN).contains(&name.len())
                && name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                }) =>
            {
                Network::Other(NetworkName(name.to_owned()))
            }
            _ => return Err(InvalidNetworkName(name.to_owned())),
        })
    }
}

impl From<&Network> for Network {
    fn from(n: &Network) -> Self {
        n.clone()
    }
}

/// The name of a network that has no variant of its own in [`Network`].
///
/// Only the parse of a [`Network`] makes one. So the name always passes the
/// rule of that parse, and it is never the name of a known network.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetworkName(String);

impl NetworkName {
    /// The literal name as used in the bech32 HRP.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetworkName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A network name that the parse of a [`Network`] refuses.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "{0:?} is not a valid network name: use 1 to {max} characters, each a lowercase ASCII \
     letter, a digit, `-` or `_`",
    max = MAX_NAME_LEN
)]
pub struct InvalidNetworkName(String);

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
            let got: Network = name.parse().unwrap();
            assert_eq!(got, want);
            assert_eq!(got.as_str(), name);
            assert_eq!(AsRef::<str>::as_ref(&got), name);
            assert_eq!(format!("{got}"), name);
        }
    }

    /// A wallet puts its network name in the HRP of its addresses, where an
    /// invalid HRP panics upstream derivation. It also puts the name in its
    /// storage path, where a leading `/` or a `..` leaves the storage dir.
    /// The longest valid name must still derive both addresses.
    #[test]
    fn only_a_name_safe_in_an_address_and_a_path_parses() {
        let longest = "a".repeat(MAX_NAME_LEN);
        let seed = crate::hd::Seed::from_bytes(&[1; 32]).unwrap();
        for name in ["preprod", "mainnet", "node-dev_01", longest.as_str()] {
            let network: Network = name.parse().unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(network.as_str(), name);
            seed.unshielded_address(&network);
            seed.shielded_address(&network);
        }
        assert!(matches!("node-dev_01".parse(), Ok(Network::Other(_))));

        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        for name in ["", "Preprod", "my net", "/abs", "..", "../x", &too_long] {
            assert!(name.parse::<Network>().is_err(), "{name:?} must be refused");
        }
    }
}
