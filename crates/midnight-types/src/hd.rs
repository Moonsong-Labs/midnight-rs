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

use bip32::{DerivationPath as Bip32DerivationPath, XPrv};
use midnight_helpers::ledger_9::{DefaultDB, IntoWalletAddress, ShieldedWallet, UnshieldedWallet};
use rand::RngCore;
use zeroize::Zeroize;

use crate::ledger_9::convert::{IntoLedger, IntoSdk};
use crate::{Network, ShieldedRecipient};

// Re-export the upstream `Role` enum so callers don't have to dig into
// `midnight_helpers::*`. The numeric indices match the [Wallet Engine
// Specification's role table][spec]; the names differ from the spec text
// but the indices are the contract:
//
//   index | spec name                 | enum variant
//   ------|---------------------------|---------------------------
//     0   | Unshielded External chain | Role::UnshieldedExternal
//     1   | Unshielded Internal chain | Role::UnshieldedInternal
//     2   | Dust                      | Role::Dust
//     3   | Shielded                  | Role::Zswap
//     4   | Metadata                  | Role::Ecdsa (upstream derives its ECDSA key here)
//
// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
pub use midnight_helpers::ledger_9::Role;

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
    /// The input was not valid hex.
    #[error("invalid hex: {0}")]
    InvalidHex(hex::FromHexError),

    /// The input had a byte length other than 16, 32, or 64.
    #[error("expected 16, 32, or 64 bytes; got {0}")]
    InvalidLength(usize),

    /// The input was not a valid BIP-39 mnemonic.
    #[error("invalid mnemonic: {0}")]
    InvalidMnemonic(bip39::Error),

    /// The input string did not parse as hex or a mnemonic.
    #[error("could not parse seed input as hex or a mnemonic")]
    Unrecognized,

    /// The supplied BIP-32 derivation path was syntactically invalid.
    #[error("invalid derivation path: {0}")]
    InvalidPath(bip32::Error),

    /// BIP-32 derivation hit a non-derivable child key (cryptographically
    /// improbable but not impossible). Increment the index and retry.
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
    pub fn from_mnemonic(phrase: &str) -> Result<Self, SeedError> {
        Self::from_mnemonic_with_passphrase(phrase, "")
    }

    /// Construct from a BIP-39 mnemonic with an explicit passphrase (the
    /// "25th word"). The passphrase is mixed into the seed via the standard
    /// PBKDF2-HMAC-SHA512 round defined by BIP-39; different passphrases on
    /// the same phrase produce entirely different seeds.
    pub fn from_mnemonic_with_passphrase(
        phrase: &str,
        passphrase: &str,
    ) -> Result<Self, SeedError> {
        let mnemonic = bip39::Mnemonic::parse(phrase).map_err(SeedError::InvalidMnemonic)?;
        Ok(Self(SeedBytes::Long(mnemonic.to_seed(passphrase))))
    }

    /// Construct from a hex string. The hex must decode to exactly 16, 32, or
    /// 64 bytes.
    pub fn from_hex(hex_str: &str) -> Result<Self, SeedError> {
        let bytes = hex::decode(hex_str).map_err(SeedError::InvalidHex)?;
        Self::from_bytes(&bytes)
    }

    /// Construct from raw entropy bytes. Length must be 16, 32, or 64.
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
    /// `network` accepts both [`Network`] and `&str` / `String` (via `Into`).
    pub fn unshielded_address(&self, network: impl Into<Network>) -> String {
        UnshieldedWallet::default(self.into_ledger())
            .address(network.into().as_str())
            .to_bech32()
    }

    /// The shielded receiving address of this seed on `network`, e.g.
    /// `mn_shield-addr_undeployed1...`.
    ///
    /// `network` accepts both [`Network`] and `&str` / `String` (via `Into`).
    pub fn shielded_address(&self, network: impl Into<Network>) -> String {
        ShieldedWallet::<DefaultDB>::default(self.into_ledger())
            .address(network.into().as_str())
            .to_bech32()
    }

    /// Derive the shielded keys of this seed: its `coin_public_key` (coin
    /// ownership) and `enc_public_key` (coin discovery).
    ///
    /// [`Self::shielded_address`] bech32-encodes the same public material into a
    /// string for sharing; this returns the typed keys so you can use them
    /// directly, e.g. to build a coin encryption mapping for a coin you mint to
    /// your own address. To recover these keys from someone else's shared
    /// address string, use [`crate::address::parse_shielded_recipient`] instead.
    pub fn shielded_recipient(&self) -> ShieldedRecipient {
        (&ShieldedWallet::<DefaultDB>::default(self.into_ledger())).into_sdk()
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
    /// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
    pub fn derive_at(self, index: u32) -> Result<[u8; 32], SeedError> {
        let role_index = role_index(self.role);
        let path = format!(
            "m/{PURPOSE}'/{COIN_TYPE}'/{}'/{}/{}",
            self.account, role_index, index
        );
        let path = Bip32DerivationPath::from_str(&path).map_err(SeedError::InvalidPath)?;
        let xprv = XPrv::derive_from_path(self.seed.as_bytes(), &path)
            .map_err(|_| SeedError::KeyOutOfBounds)?;
        Ok(xprv.private_key().to_bytes().into())
    }
}

/// Map upstream's `Role` to the numeric child index pinned by the
/// [Wallet Engine Specification's role table][spec]. Same indices upstream's
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
/// [`mnemonic::generate`] defaults to 256 bits of entropy (24 words),
/// [`mnemonic::validate`] runs the BIP-39 checksum, and [`mnemonic::words`] /
/// [`mnemonic::join`] split / re-join the phrase on whitespace.
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
        /// 256 bits → 24 words. Default.
        Words24,
    }

    impl Strength {
        fn bits(self) -> usize {
            match self {
                Strength::Words12 => 128,
                Strength::Words15 => 160,
                Strength::Words18 => 192,
                Strength::Words21 => 224,
                Strength::Words24 => 256,
            }
        }
    }

    /// Generate a fresh mnemonic phrase. `Strength::Words24` (256 bits of
    /// entropy) is the default for new wallets.
    pub fn generate(strength: Strength) -> Vec<String> {
        // `bip39::Mnemonic::generate` takes a word count, not an entropy
        // width, but the mapping is exact (32 bits per 3 words after the
        // ceil-divide BIP-39 spec). We forward the bit count and let it
        // pick the right word count internally.
        let word_count = match strength {
            Strength::Words12 => 12,
            Strength::Words15 => 15,
            Strength::Words18 => 18,
            Strength::Words21 => 21,
            Strength::Words24 => 24,
        };
        let _ = strength.bits(); // documented bit width matches word_count
        let mnemonic = bip39::Mnemonic::generate(word_count)
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
        use midnight_helpers::ledger_9::{DerivationPath, DeriveSeed, UnshieldedWallet};

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
            .map(|role| (0, role.clone(), 0, DerivationPath::default_for_role(role)))
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
        assert_eq!(seed.shielded_recipient(), (&shielded8).into_sdk());
    }

    #[test]
    fn unshielded_address_uses_network_suffix() {
        let addr = seed().unshielded_address("undeployed");
        assert!(addr.starts_with("mn_addr_undeployed"), "address was {addr}");
    }

    #[test]
    fn shielded_address_uses_network_suffix() {
        let addr = seed().shielded_address("undeployed");
        assert!(
            addr.starts_with("mn_shield-addr_undeployed"),
            "address was {addr}"
        );
    }

    #[test]
    fn unshielded_address_differs_per_network() {
        let a = seed().unshielded_address("undeployed");
        let b = seed().unshielded_address("testnet");
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
