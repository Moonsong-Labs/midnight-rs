//! Malformed and cross-network address inputs must produce typed errors, not
//! panics and not silently-accepted recipients. Both hazards live upstream:
//! `TryFrom<&WalletAddress> for ShieldedWallet` asserts the payload length
//! instead of returning its `InvalidCoinKeyLen` variant, and it validates the
//! HRP's prefix and credential segments but never its network segment.
//! `TryFrom<&WalletAddress>` for `DustWallet` and for `UnshieldedWallet` also
//! never checks the network segment.

use midnight_wallet::{
    Network, Seed, WalletError, parse_dust_address, parse_shielded_recipient,
    parse_unshielded_recipient,
};

fn seed() -> Seed {
    Seed::from_hex("0000000000000000000000000000000000000000000000000000000000000001").unwrap()
}

#[test]
fn short_shielded_address_is_an_error_not_a_panic() {
    let s = "mn_shield-addr_undeployed1qqqqqqqqqqqqqrnvycf";
    let r = std::panic::catch_unwind(|| parse_shielded_recipient(s, Network::Undeployed));
    let r = r.expect("truncated address must not panic");
    assert!(r.is_err(), "truncated address must be rejected");
}

#[test]
fn cross_network_address_is_rejected() {
    let rows = [
        (
            "shielded",
            parse_shielded_recipient(
                &seed().shielded_address(Network::Testnet),
                Network::Undeployed,
            )
            .err(),
        ),
        (
            "dust",
            parse_dust_address(&seed().dust_address(Network::Testnet), Network::Undeployed).err(),
        ),
        (
            "unshielded",
            parse_unshielded_recipient(
                &seed().unshielded_address(Network::Testnet),
                Network::Undeployed,
            )
            .err(),
        ),
    ];
    for (kind, err) in rows {
        assert!(
            matches!(
                &err,
                Some(WalletError::AddressNetworkMismatch { expected, actual })
                    if expected == "undeployed" && actual == "testnet"
            ),
            "{kind}: got {err:?}"
        );
    }
}

#[test]
fn mainnet_has_no_hrp_network_suffix() {
    // Upstream's `network_suffix` returns an empty string for mainnet, so a
    // mainnet address is `mn_shield-addr1...` with no third HRP segment. The
    // check has to treat that absence as "mainnet" rather than "unknown".
    let addr = seed().shielded_address(Network::Mainnet);
    assert!(
        !addr.starts_with("mn_shield-addr_"),
        "mainnet address should carry no network suffix, got: {addr}"
    );
    assert!(parse_shielded_recipient(&addr, Network::Mainnet).is_ok());
    assert!(
        parse_shielded_recipient(&addr, Network::Testnet).is_err(),
        "a mainnet address must not be accepted by a testnet wallet"
    );
    let testnet = seed().shielded_address(Network::Testnet);
    assert!(
        parse_shielded_recipient(&testnet, Network::Mainnet).is_err(),
        "a testnet address must not be accepted by a mainnet wallet"
    );
}

/// The network is the HRP's *last* segment, not its third: upstream builds the
/// HRP as `mn_shield-addr_<network>`, so a network name containing `_` produces
/// more than three underscore-separated pieces.
#[test]
fn custom_network_names_may_contain_underscores() {
    let network: Network = "custom_devnet".parse().unwrap();
    let addr = seed().shielded_address(network.clone());
    assert!(
        parse_shielded_recipient(&addr, network).is_ok(),
        "an address derived for `custom_devnet` must parse under that network"
    );
}
