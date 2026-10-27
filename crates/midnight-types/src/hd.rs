//! HD wallet primitives: seeds, mnemonics, and BIP-32 derivation.
//!
//! Implements the HD-wallet layout from Midnight's [Wallet Engine Specification, *HD wallet structure*](https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure): a mix of BIP-32, BIP-44, and CIP-1852 with the path `m / purpose' / coin_type' / account' / role / index`, `purpose = 44`, `coin_type = 2400`, and five role indices (0 Unshielded External chain, 1 Unshielded Internal chain, 2 Dust, 3 Shielded, 4 Metadata).
//!
//! # Quick start
//!
//! ```no_run
//! use midnight_types::hd::{Role, Seed};
//!
//! // Empty passphrase.
//! let seed = Seed::from_mnemonic(
//!     "abandon abandon abandon abandon abandon abandon abandon abandon \
//!      abandon abandon abandon abandon abandon abandon abandon abandon \
//!      abandon abandon abandon abandon abandon abandon abandon diesel",
//! ).unwrap();
//!
//! // The default sync and the address methods use the standard per-asset
//! // accounts (account 0, the role's default leaf). For an explicit key:
//! let key: [u8; 32] = seed
//!     .account(0)
//!     .role(Role::Zswap)
//!     .derive_at(0)
//!     .unwrap();
//! ```
//!
//! # Mnemonic utilities
//!
//! [`mnemonic::generate`] produces a fresh phrase, [`mnemonic::validate`]
//! checks one, and [`mnemonic::words`] / [`mnemonic::join`] convert between
//! the wire-form phrase string and a `Vec<String>` for UIs that want to render
//! one word per slot.

use std::fmt;
use std::str::FromStr;

use bip32::{ChildNumber, DerivationPath as Bip32DerivationPath, XPrv};
use midnight_helpers::ledger_9::{DefaultDB, IntoWalletAddress, ShieldedWallet, UnshieldedWallet};
use rand::RngCore;
use zeroize::Zeroize;

use crate::ledger_9::convert::{IntoLedger, IntoSdk};
use crate::{Network, ShieldedRecipient, UserAddress};

/// The `role` level of an HD path: the asset family a key belongs to.
///
/// The index of each role is the contract of the [Wallet Engine
/// Specification's role table][spec]. The variant names follow the upstream
/// ledger helpers, and some differ from the names in the spec text.
///
/// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Index 0, the spec's "Unshielded External chain".
    UnshieldedExternal,
    /// Index 1, the spec's "Unshielded Internal chain".
    UnshieldedInternal,
    /// Index 2, the spec's "Dust".
    Dust,
    /// Index 3, the spec's "Shielded".
    Zswap,
    /// Index 4, the spec's "Metadata", where upstream derives its ECDSA key.
    Ecdsa,
}

/// BIP-44 `purpose` level. Constant per the BIP-44 spec; the [Wallet Engine
/// Specification][spec] pins this at `44` (`0x8000002c` once hardened).
///
/// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
const PURPOSE: u32 = 44;

/// BIP-44 `coin_type` slot Midnight uses. Defined by the [Wallet Engine
/// Specification][spec] (`0x80000960` once hardened); not registered in
/// SLIP-44, the value lives only in code. Upstream `midnight-node-ledger-helpers`
/// hard-codes the same `2400` in its `m/44'/2400'/0'/<role>/0` strings.
///
/// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
const COIN_TYPE: u32 = 2400;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced when constructing or deriving from a [`Seed`].
#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// [`Seed::from_hex`] got a string that is not hex. The value is the
    /// decoder's message.
    #[error("invalid hex: {0}")]
    InvalidHex(String),

    /// The seed bytes have a length other than 16, 32 or 64. The value is
    /// that length.
    #[error("expected 16, 32, or 64 bytes; got {0}")]
    InvalidLength(usize),

    /// The phrase is not a valid BIP-39 mnemonic in the English word list.
    /// The value is the parser's message.
    #[error("invalid mnemonic: {0}")]
    InvalidMnemonic(String),

    /// [`Seed::from_str`] found neither hex of 16, 32 or 64 bytes nor a valid
    /// mnemonic.
    #[error("could not parse seed input as hex or a mnemonic")]
    Unrecognized,

    /// [`RoleKey::derive_at`] got an account or an index of 2^31 or above.
    /// The value is the first such number. BIP-32 keeps those child numbers
    /// for hardened children.
    #[error("account or index {0} is 2^31 or above, which BIP-32 keeps for hardened children")]
    IndexOutOfRange(u32),

    /// The BIP-32 derivation gave an invalid key at one level of the path.
    /// BIP-32 puts the chance below 1 in 2^127 at each level, and tells the
    /// caller to go on with the next index.
    #[error("derived key out of range at child index")]
    KeyOutOfBounds,
}

// ---------------------------------------------------------------------------
// Seed
// ---------------------------------------------------------------------------

/// A wallet seed. Holds the root entropy for BIP-32 HD derivation.
///
/// The bytes can be 16, 32, or 64 wide. `as_bytes` exposes them regardless
/// of width. BIP-32 reads the slice, so the width changes nothing downstream.
///
/// Every ledger generation derives the same keys and addresses from a seed,
/// so the address methods derive through ledger 9.
///
/// `Drop` zeroizes the seed bytes. `Debug` and `Display` redact them.
#[derive(Clone, PartialEq, Eq)]
pub struct Seed(pub(crate) SeedBytes);

/// The entropy of a [`Seed`], in one of the three widths it accepts.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SeedBytes {
    Short([u8; 16]),
    Medium([u8; 32]),
    Long([u8; 64]),
}

impl Seed {
    /// Construct from a BIP-39 mnemonic phrase with an empty passphrase.
    ///
    /// Runs the BIP-39 standard derivation (`Mnemonic::to_seed("")`,
    /// PBKDF2-HMAC-SHA512 with 2048 rounds) and produces a 64-byte seed.
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidMnemonic`] when `phrase` is not a valid BIP-39
    /// mnemonic.
    pub fn from_mnemonic(phrase: &str) -> Result<Self, SeedError> {
        Self::from_mnemonic_with_passphrase(phrase, "")
    }

    /// Construct from a BIP-39 mnemonic with an explicit passphrase (the
    /// "25th word"). The passphrase is mixed into the seed via the standard
    /// PBKDF2-HMAC-SHA512 round defined by BIP-39; different passphrases on
    /// the same phrase produce entirely different seeds.
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidMnemonic`] when `phrase` is not a valid BIP-39
    /// mnemonic.
    pub fn from_mnemonic_with_passphrase(
        phrase: &str,
        passphrase: &str,
    ) -> Result<Self, SeedError> {
        let mnemonic = bip39::Mnemonic::parse(phrase)
            .map_err(|e| SeedError::InvalidMnemonic(e.to_string()))?;
        Ok(Self(SeedBytes::Long(mnemonic.to_seed(passphrase))))
    }

    /// Construct from a hex string. The hex must decode to exactly 16, 32, or
    /// 64 bytes.
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidHex`] when `hex_str` is not hex, and
    /// [`SeedError::InvalidLength`] when it decodes to another length.
    pub fn from_hex(hex_str: &str) -> Result<Self, SeedError> {
        let bytes = hex::decode(hex_str).map_err(|e| SeedError::InvalidHex(e.to_string()))?;
        Self::from_bytes(&bytes)
    }

    /// Construct from raw entropy bytes. Length must be 16, 32, or 64.
    ///
    /// # Errors
    ///
    /// [`SeedError::InvalidLength`] when `bytes` has another length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SeedError> {
        let inner = if let Ok(short) = bytes.try_into() {
            SeedBytes::Short(short)
        } else if let Ok(medium) = bytes.try_into() {
            SeedBytes::Medium(medium)
        } else if let Ok(long) = bytes.try_into() {
            SeedBytes::Long(long)
        } else {
            return Err(SeedError::InvalidLength(bytes.len()));
        };
        Ok(Self(inner))
    }

    /// Generate a fresh 32-byte seed from a cryptographically secure RNG.
    /// For mnemonic-backed wallets, use [`mnemonic::generate`] +
    /// [`Self::from_mnemonic`] so the user can write the phrase down.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(SeedBytes::Medium(bytes))
    }

    /// The seed's raw bytes (16, 32, or 64).
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            SeedBytes::Short(bytes) => bytes,
            SeedBytes::Medium(bytes) => bytes,
            SeedBytes::Long(bytes) => bytes,
        }
    }

    /// Entry point for explicit HD key derivation. The default wallet sync
    /// and the address methods already use the standard per-asset accounts;
    /// reach for this when you need a specific
    /// `m/44'/2400'/<account>'/<role>/<index>` key.
    pub fn account(&self, account: u32) -> AccountKey<'_> {
        AccountKey {
            seed: self,
            account,
        }
    }

    /// The unshielded receiving address of this seed on `network`, e.g.
    /// `mn_addr_undeployed1...`.
    ///
    /// `network` is a [`Network`] or a `&Network`.
    pub fn unshielded_address(&self, network: impl Into<Network>) -> String {
        UnshieldedWallet::default(self.into_ledger())
            .address(network.into().as_str())
            .to_bech32()
    }

    /// The user address of this seed: the owner of its unshielded UTXOs, and
    /// the bytes that [`Self::unshielded_address`] carries.
    ///
    /// It takes no network, because the bytes are the same on every network.
    /// Give it to a Compact circuit that takes a `UserAddress`. To recover a
    /// user address from someone else's shared address string, use
    /// [`crate::address::parse_unshielded_address`] instead.
    pub fn user_address(&self) -> UserAddress {
        UnshieldedWallet::default(self.into_ledger())
            .user_address
            .into_sdk()
    }

    /// The shielded receiving address of this seed on `network`, e.g.
    /// `mn_shield-addr_undeployed1...`.
    ///
    /// `network` is a [`Network`] or a `&Network`.
    pub fn shielded_address(&self, network: impl Into<Network>) -> String {
        ShieldedWallet::<DefaultDB>::default(self.into_ledger())
            .address(network.into().as_str())
            .to_bech32()
    }

    /// Derive the shielded keys of this seed: its `coin_public_key` (coin
    /// ownership) and `enc_public_key` (coin discovery).
    ///
    /// [`Self::shielded_address`] bech32-encodes the same public material into a
    /// string for sharing. This returns the typed keys for direct use, e.g. as
    /// the recipient of a coin that a circuit mints. Give that recipient to
    /// `with_coin_encryption_keys`. To recover these keys from someone else's
    /// shared address string, use [`crate::address::parse_shielded_recipient`]
    /// instead.
    pub fn shielded_recipient(&self) -> ShieldedRecipient {
        crate::address::shielded_recipient(&ShieldedWallet::default(self.into_ledger()))
    }
}

/// Clones the seed, so an API that takes `impl Into<Seed>` also takes a
/// `&Seed` and leaves the original with the caller.
impl From<&Seed> for Seed {
    fn from(seed: &Seed) -> Self {
        seed.clone()
    }
}

impl FromStr for Seed {
    type Err = SeedError;

    /// Tries hex, then a mnemonic, in that order.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(seed) = Self::from_hex(s) {
            return Ok(seed);
        }
        if let Ok(seed) = Self::from_mnemonic(s) {
            return Ok(seed);
        }
        Err(SeedError::Unrecognized)
    }
}

impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Seed(REDACTED)")
    }
}

impl fmt::Display for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "REDACTED")
    }
}

impl Drop for Seed {
    /// Zeroize the seed bytes on drop. This clears the entropy and keeps the
    /// width.
    fn drop(&mut self) {
        match &mut self.0 {
            SeedBytes::Short(bytes) => bytes.zeroize(),
            SeedBytes::Medium(bytes) => bytes.zeroize(),
            SeedBytes::Long(bytes) => bytes.zeroize(),
        }
    }
}

// ---------------------------------------------------------------------------
// HD derivation builder: AccountKey -> RoleKey -> derive_at
// ---------------------------------------------------------------------------

/// Step 2 of the `seed.account(n).role(r).derive_at(i)` builder. Constructed
/// via [`Seed::account`].
#[derive(Debug)]
pub struct AccountKey<'a> {
    seed: &'a Seed,
    account: u32,
}

impl<'a> AccountKey<'a> {
    /// Select the role (asset family) within this account.
    pub fn role(self, role: Role) -> RoleKey<'a> {
        RoleKey {
            seed: self.seed,
            account: self.account,
            role,
        }
    }
}

/// Step 3 of the builder. Resolved by calling [`Self::derive_at`].
#[derive(Debug)]
pub struct RoleKey<'a> {
    seed: &'a Seed,
    account: u32,
    role: Role,
}

impl<'a> RoleKey<'a> {
    /// Derive the child private key at `m / 44' / 2400' / <account>' / <role> / <index>`,
    /// the path defined in the [Wallet Engine Specification][spec]. The
    /// 32-byte result is the raw private-key material the chain's signing /
    /// encryption schemes consume; it is **not** an address.
    ///
    /// # Errors
    ///
    /// [`SeedError::IndexOutOfRange`] when the account or `index` is 2^31 or
    /// above, and [`SeedError::KeyOutOfBounds`] when BIP-32 gives an invalid
    /// key on the path.
    ///
    /// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
    pub fn derive_at(self, index: u32) -> Result<[u8; 32], SeedError> {
        for n in [self.account, index] {
            if n >= ChildNumber::HARDENED_FLAG {
                return Err(SeedError::IndexOutOfRange(n));
            }
        }
        let role_index = role_index(self.role);
        let path = format!(
            "m/{PURPOSE}'/{COIN_TYPE}'/{}'/{}/{}",
            self.account, role_index, index
        );
        let path = Bip32DerivationPath::from_str(&path)
            .expect("every child number of the path is below 2^31, so the path parses");
        let xprv = XPrv::derive_from_path(self.seed.as_bytes(), &path)
            .map_err(|_| SeedError::KeyOutOfBounds)?;
        Ok(xprv.private_key().to_bytes().into())
    }
}

/// Map a [`Role`] to the numeric child index pinned by the [Wallet Engine
/// Specification's role table][spec]. Same indices upstream's
/// `DerivationPath::default_for_role` builds into its
/// `m/44'/2400'/0'/<index>/0` strings.
///
/// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
fn role_index(role: Role) -> u32 {
    match role {
        Role::UnshieldedExternal => 0,
        Role::UnshieldedInternal => 1,
        Role::Dust => 2,
        Role::Zswap => 3,
        Role::Ecdsa => 4,
    }
}

// ---------------------------------------------------------------------------
// Mnemonic utilities
// ---------------------------------------------------------------------------

/// Mnemonic phrase generation, validation, and word-list conversion.
///
/// [`mnemonic::generate`] makes a phrase of the [`mnemonic::Strength`] you
/// choose, [`mnemonic::validate`] runs the BIP-39 checksum, and
/// [`mnemonic::words`] / [`mnemonic::join`] split / re-join the phrase on
/// whitespace.
pub mod mnemonic {
    /// Mnemonic phrase length, by entropy width. 24 words (256 bits) is the
    /// standard BIP-39 strong choice.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Strength {
        /// 128 bits → 12 words.
        Words12,
        /// 160 bits → 15 words.
        Words15,
        /// 192 bits → 18 words.
        Words18,
        /// 224 bits → 21 words.
        Words21,
        /// 256 bits → 24 words.
        Words24,
    }

    impl Strength {
        fn word_count(self) -> usize {
            match self {
                Strength::Words12 => 12,
                Strength::Words15 => 15,
                Strength::Words18 => 18,
                Strength::Words21 => 21,
                Strength::Words24 => 24,
            }
        }
    }

    /// Generate a fresh mnemonic phrase. `Strength::Words24` (256 bits of
    /// entropy) is the usual choice for a new wallet.
    pub fn generate(strength: Strength) -> Vec<String> {
        let mnemonic = bip39::Mnemonic::generate(strength.word_count())
            .expect("bip39 generate accepts 12/15/18/21/24 word counts");
        mnemonic.words().map(str::to_string).collect()
    }

    /// `true` if the phrase parses as a valid BIP-39 mnemonic in the
    /// English wordlist.
    pub fn validate(phrase: &str) -> bool {
        bip39::Mnemonic::parse(phrase).is_ok()
    }

    /// Split a whitespace-separated phrase into individual words. Doesn't
    /// validate the phrase: that's [`validate`]'s job.
    pub fn words(phrase: &str) -> Vec<&str> {
        phrase.split_whitespace().collect()
    }

    /// Re-join a word vector into a single phrase string with single spaces.
    pub fn join(words: &[String]) -> String {
        words.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical 24-word "abandon … diesel" mnemonic from the BIP-39 test
    /// vectors, also pinned by upstream `midnight-node-ledger-helpers`'s own
    /// test. The expected 64-byte hex below comes verbatim from that test
    /// (`ledger/helpers/src/versions/common/types.rs`,
    /// `should_decode_wallet_seeds_in_different_formats`).
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon diesel";
    const TEST_SEED_HEX: &str = "a51c86de32d0791f7cffc3bdff1abd9bb54987f0ed5effc30c936dddbb9afd9d530c8db445e4f2d3ea42a321b260e022aadf05987c9a67ec7b6b6ca1d0593ec9";

    fn seed() -> Seed {
        Seed::from_bytes(&[1u8; 32]).unwrap()
    }

    #[test]
    fn from_mnemonic_matches_upstream_test_vector() {
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        assert_eq!(hex::encode(seed.as_bytes()), TEST_SEED_HEX);
    }

    #[test]
    fn passphrase_changes_the_seed() {
        let bare = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        let with_pw = Seed::from_mnemonic_with_passphrase(TEST_MNEMONIC, "trezor").unwrap();
        assert_ne!(
            bare.as_bytes(),
            with_pw.as_bytes(),
            "different passphrases must produce different seeds",
        );
    }

    #[test]
    fn from_bytes_accepts_16_32_64_only() {
        assert!(Seed::from_bytes(&[0u8; 16]).is_ok());
        assert!(Seed::from_bytes(&[0u8; 32]).is_ok());
        assert!(Seed::from_bytes(&[0u8; 64]).is_ok());
        assert!(matches!(
            Seed::from_bytes(&[0u8; 17]),
            Err(SeedError::InvalidLength(17)),
        ));
        assert!(matches!(
            Seed::from_bytes(&[0u8; 33]),
            Err(SeedError::InvalidLength(33)),
        ));
    }

    #[test]
    fn derive_at_refuses_a_hardened_account_or_index() {
        let seed = seed();
        let zswap = |account| seed.account(account).role(Role::Zswap);
        assert!(matches!(
            zswap(0).derive_at(1 << 31),
            Err(SeedError::IndexOutOfRange(n)) if n == 1 << 31,
        ));
        assert!(zswap(0).derive_at((1 << 31) - 1).is_ok());
        assert!(matches!(
            zswap(1 << 31).derive_at(0),
            Err(SeedError::IndexOutOfRange(n)) if n == 1 << 31,
        ));
    }

    #[test]
    fn from_str_tries_hex_then_mnemonic() {
        // hex
        let s1: Seed = TEST_SEED_HEX.parse().unwrap();
        assert_eq!(hex::encode(s1.as_bytes()), TEST_SEED_HEX);
        // mnemonic
        let s2: Seed = TEST_MNEMONIC.parse().unwrap();
        assert_eq!(hex::encode(s2.as_bytes()), TEST_SEED_HEX);
        // gibberish
        let s3: Result<Seed, _> = "not a seed at all".parse();
        assert!(matches!(s3, Err(SeedError::Unrecognized)));
    }

    #[test]
    fn debug_and_display_redact() {
        let seed = Seed::from_hex(TEST_SEED_HEX).unwrap();
        let dbg = format!("{seed:?}");
        let disp = format!("{seed}");
        // Neither rendering should leak any byte of the seed.
        assert!(dbg.contains("REDACTED"), "{dbg}");
        assert!(disp.contains("REDACTED"), "{disp}");
        assert!(
            !dbg.contains("a51c"),
            "Debug must not leak seed bytes: {dbg}"
        );
        assert!(!disp.contains("a51c"), "Display must not leak: {disp}");
    }

    // Upstream writes its role paths independently of `role_index`, and its
    // wallets derive their keys from those paths.
    #[test]
    fn derive_at_matches_upstream_derivation_paths() {
        use midnight_helpers::ledger_9::{self, DerivationPath, DeriveSeed, UnshieldedWallet};

        let upstream_role = |role| match role {
            Role::UnshieldedExternal => ledger_9::Role::UnshieldedExternal,
            Role::UnshieldedInternal => ledger_9::Role::UnshieldedInternal,
            Role::Dust => ledger_9::Role::Dust,
            Role::Zswap => ledger_9::Role::Zswap,
            Role::Ecdsa => ledger_9::Role::Ecdsa,
        };
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        let path = |p: &str| DerivationPath::new(p.to_string()).unwrap();
        let roles = [
            Role::UnshieldedExternal,
            Role::UnshieldedInternal,
            Role::Dust,
            Role::Zswap,
            Role::Ecdsa,
        ];
        let rows = roles
            .into_iter()
            .map(|role| {
                let upstream_path = DerivationPath::default_for_role(upstream_role(role));
                (0, role, 0, upstream_path)
            })
            .chain([
                (1, Role::Zswap, 0, path("m/44'/2400'/1'/3/0")),
                (0, Role::Zswap, 1, path("m/44'/2400'/0'/3/1")),
            ]);

        for (account, role, index, upstream_path) in rows {
            let expected = <UnshieldedWallet as DeriveSeed>::derive_seed(
                crate::ledger_9::convert::IntoLedger::into_ledger(&seed),
                &upstream_path,
            );
            assert_eq!(
                seed.account(account).role(role).derive_at(index).unwrap(),
                expected,
                "key for {}",
                upstream_path.path
            );
        }
    }

    /// The address methods derive through ledger 9 alone. If ledger 8 derived
    /// other keys from the same seed, a wallet on a ledger 8 chain would
    /// receive at addresses its owner never sees.
    #[test]
    fn both_generations_derive_the_same_addresses_and_keys() {
        use crate::ledger_8::convert::IntoSdk as _;
        use midnight_helpers::ledger_8;
        use midnight_helpers::ledger_8::IntoWalletAddress as _;

        let seed = seed();
        let seed8 = crate::ledger_8::convert::IntoLedger::into_ledger(&seed);
        let unshielded8 = ledger_8::UnshieldedWallet::default(seed8.clone())
            .address(Network::Undeployed.as_str())
            .to_bech32();
        let shielded8 = ledger_8::ShieldedWallet::<ledger_8::DefaultDB>::default(seed8);
        let address8 = shielded8.address(Network::Undeployed.as_str()).to_bech32();

        assert_eq!(seed.unshielded_address(Network::Undeployed), unshielded8);
        assert_eq!(seed.shielded_address(Network::Undeployed), address8);
        let recipient8 = ShieldedRecipient {
            coin_public_key: shielded8.coin_public_key.into_sdk(),
            enc_public_key: shielded8.enc_public_key.into_sdk(),
        };
        assert_eq!(seed.shielded_recipient(), recipient8);
    }

    #[test]
    fn unshielded_address_uses_network_suffix() {
        let addr = seed().unshielded_address(Network::Undeployed);
        assert!(addr.starts_with("mn_addr_undeployed"), "address was {addr}");
    }

    #[test]
    fn shielded_address_uses_network_suffix() {
        let addr = seed().shielded_address(Network::Undeployed);
        assert!(
            addr.starts_with("mn_shield-addr_undeployed"),
            "address was {addr}"
        );
    }

    #[test]
    fn unshielded_address_differs_per_network() {
        let a = seed().unshielded_address(Network::Undeployed);
        let b = seed().unshielded_address(Network::Testnet);
        assert_ne!(a, b);
    }

    #[test]
    fn mnemonic_generate_validates() {
        let words = mnemonic::generate(mnemonic::Strength::Words24);
        assert_eq!(words.len(), 24);
        let phrase = mnemonic::join(&words);
        assert!(mnemonic::validate(&phrase));
        // Round-trip into a seed and back to a mnemonic-derived seed.
        let seed = Seed::from_mnemonic(&phrase).unwrap();
        assert_eq!(seed.as_bytes().len(), 64);
    }

    #[test]
    fn mnemonic_validate_rejects_garbage() {
        assert!(!mnemonic::validate("not a real mnemonic phrase at all"));
        // Wrong-length phrase
        assert!(!mnemonic::validate("abandon abandon abandon"));
    }

    #[test]
    fn mnemonic_words_and_join_round_trip() {
        let phrase = TEST_MNEMONIC;
        let words: Vec<String> = mnemonic::words(phrase)
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(words.len(), 24);
        assert_eq!(mnemonic::join(&words), phrase);
    }
}
