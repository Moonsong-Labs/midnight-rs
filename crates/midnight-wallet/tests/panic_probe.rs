//! Malformed and cross-network address inputs must produce typed errors, not
//! panics and not silently-accepted recipients. Both hazards live upstream:
//! `TryFrom<&WalletAddress> for ShieldedWallet` asserts the payload length
//! instead of returning its `InvalidCoinKeyLen` variant, and it validates the
//! HRP's prefix and credential segments but never its network segment.

use midnight_wallet::{Network, parse_shielded_recipient};

fn seed() -> midnight_helpers::WalletSeed {
    midnight_helpers::WalletSeed::try_from_hex_str(
        "0000000000000000000000000000000000000000000000000000000000000001",
    )
    .unwrap()
}

#[test]
fn short_shielded_address_is_an_error_not_a_panic() {
    let s = "mn_shield-addr_undeployed1qqqqqqqqqqqqqrnvycf";
    let r = std::panic::catch_unwind(|| parse_shielded_recipient(s, Network::Undeployed));
    let r = r.expect("truncated address must not panic");
    assert!(r.is_err(), "truncated address must be rejected");
}

#[test]
fn cross_network_shielded_address_is_rejected() {
    let testnet = midnight_wallet::address::derive_shielded(&seed(), Network::Testnet);
    let err = parse_shielded_recipient(&testnet, Network::Undeployed)
        .expect_err("a testnet address must not be accepted by an undeployed wallet");
    let msg = err.to_string();
    assert!(
        msg.contains("testnet") && msg.contains("undeployed"),
        "error should name both networks, got: {msg}"
    );
}

#[test]
fn matching_network_shielded_address_is_accepted() {
    for network in [Network::Undeployed, Network::Testnet, Network::Preprod] {
        let addr = midnight_wallet::address::derive_shielded(&seed(), network.clone());
        assert!(
            parse_shielded_recipient(&addr, network.clone()).is_ok(),
            "address derived for {network} should parse under {network}"
        );
    }
}

#[test]
fn mainnet_has_no_hrp_network_suffix() {
    // Upstream's `network_suffix` returns an empty string for mainnet, so a
    // mainnet address is `mn_shield-addr1...` with no third HRP segment. The
    // check has to treat that absence as "mainnet" rather than "unknown".
    let addr = midnight_wallet::address::derive_shielded(&seed(), Network::Mainnet);
    assert!(
        !addr.starts_with("mn_shield-addr_"),
        "mainnet address should carry no network suffix, got: {addr}"
    );
    assert!(parse_shielded_recipient(&addr, Network::Mainnet).is_ok());
    assert!(
        parse_shielded_recipient(&addr, Network::Testnet).is_err(),
        "a mainnet address must not be accepted by a testnet wallet"
    );
    let testnet = midnight_wallet::address::derive_shielded(&seed(), Network::Testnet);
    assert!(
        parse_shielded_recipient(&testnet, Network::Mainnet).is_err(),
        "a testnet address must not be accepted by a mainnet wallet"
    );
}

#[test]
fn custom_network_names_round_trip() {
    let network = Network::Other("custom-devnet".into());
    let addr = midnight_wallet::address::derive_shielded(&seed(), network.clone());
    assert!(parse_shielded_recipient(&addr, network).is_ok());
    assert!(parse_shielded_recipient(&addr, Network::Undeployed).is_err());
}
