//! HD wallet primitives: seeds, mnemonics, and BIP-32 derivation.
//!
//! Implements the HD-wallet layout from Midnight's [Wallet Engine Specification — *HD wallet structure*](https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure): a mix of BIP-32, BIP-44, and CIP-1852 with the path `m / purpose' / coin_type' / account' / role / index`, `purpose = 44`, `coin_type = 2400`, and five role indices (0 Unshielded External chain, 1 Unshielded Internal chain, 2 Dust, 3 Shielded, 4 Metadata).
//!
//! # Quick start
//!
//! ```no_run
//! use midnight_wallet::hd::{Seed, Role};
//!
//! // Empty passphrase.
//! let seed = Seed::from_mnemonic(
//!     "abandon abandon abandon abandon abandon abandon abandon abandon \
//!      abandon abandon abandon abandon abandon abandon abandon abandon \
//!      abandon abandon abandon abandon abandon abandon abandon diesel",
//! ).unwrap();
//!
//! // The default `sync_wallet` / `derive_*` paths use the standard per-asset
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
//!
//! # Compatibility with upstream `WalletSeed`
//!
//! `Seed` is a thin wrapper around upstream's `WalletSeed` enum. It implements
//! `From<Seed> for WalletSeed`, and re-exports the upstream `Role` enum from
//! `midnight_helpers`. SDK methods that take `impl Into<WalletSeed>` accept
//! both types, so callers can migrate at their own pace.

use std::fmt;
use std::str::FromStr;

use bip32::{DerivationPath as Bip32DerivationPath, XPrv};
use midnight_helpers::WalletSeed;
use rand::RngCore;
use zeroize::Zeroize;

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
//     4   | Metadata                  | Role::Metadata
//
// [spec]: https://github.com/midnightntwrk/midnight-architecture/blob/main/components/WalletEngine/Specification.md#hd-wallet-structure
pub use midnight_helpers::Role;

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

    /// The input string did not parse as hex, lazy hex, or a mnemonic.
    #[error("could not parse seed input as hex, lazy hex, or mnemonic")]
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
// Seed — newtype wrapper around upstream WalletSeed
// ---------------------------------------------------------------------------

/// A wallet seed. Holds the root entropy for BIP-32 HD derivation.
///
/// The actual bytes can be 16, 32, or 64 wide (mirroring upstream's
/// `WalletSeed::{Short, Medium, Long}` variants). `as_bytes` exposes them
/// regardless of variant; everything downstream feeds the slice to BIP-32 so
/// the width is transparent.
///
/// `Drop` zeroizes the seed bytes. `Debug` and `Display` redact them.
#[derive(Clone, PartialEq, Eq)]
pub struct Seed(WalletSeed);

impl Seed {
    /// Construct from a BIP-39 mnemonic phrase with an empty passphrase.
    ///
    /// Runs the BIP-39 standard derivation (`Mnemonic::to_seed("")`,
    /// PBKDF2-HMAC-SHA512 with 2048 rounds) and produces a 64-byte
    /// [`WalletSeed::Long`].
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
        Ok(Self(WalletSeed::Long(mnemonic.to_seed(passphrase))))
    }

    /// Construct from a hex string. The hex must decode to exactly 16, 32, or
    /// 64 bytes (`Short`, `Medium`, or `Long`).
    pub fn from_hex(hex_str: &str) -> Result<Self, SeedError> {
        let bytes = hex::decode(hex_str).map_err(SeedError::InvalidHex)?;
        Self::from_bytes(&bytes)
    }

    /// Construct from raw entropy bytes. Length must be 16, 32, or 64.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SeedError> {
        WalletSeed::try_from(bytes)
            .map(Self)
            .map_err(|_| SeedError::InvalidLength(bytes.len()))
    }

    /// Generate a fresh 32-byte seed from a cryptographically secure RNG.
    /// For mnemonic-backed wallets, use [`mnemonic::generate`] +
    /// [`Self::from_mnemonic`] so the user can write the phrase down.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(WalletSeed::Medium(bytes))
    }

    /// The seed's raw bytes (16, 32, or 64).
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Entry point for explicit HD key derivation. The default `sync_wallet`
    /// and `derive_*` paths already use the standard per-asset accounts;
    /// reach for this when you need a specific
    /// `m/44'/2400'/<account>'/<role>/<index>` key.
    pub fn account(&self, account: u32) -> AccountKey<'_> {
        AccountKey {
            seed: self,
            account,
        }
    }

    /// Derive the unshielded receiving address for this seed on `network`.
    /// Convenience wrapper over [`crate::address::derive_unshielded`].
    pub fn unshielded_address(&self, network: impl Into<crate::Network>) -> String {
        let ws: WalletSeed = self.into();
        crate::address::derive_unshielded(&ws, network)
    }

    /// Derive the shielded receiving address for this seed on `network`.
    /// Convenience wrapper over [`crate::address::derive_shielded`].
    pub fn shielded_address(&self, network: impl Into<crate::Network>) -> String {
        let ws: WalletSeed = self.into();
        crate::address::derive_shielded(&ws, network)
    }
}

impl From<Seed> for WalletSeed {
    fn from(seed: Seed) -> Self {
        // Take ownership of the inner WalletSeed without running our Drop
        // (which would zeroize it). The caller is now responsible for the
        // bytes.
        let inner = seed.0.clone();
        // Prevent our Drop from running so the caller gets a usable seed.
        std::mem::forget(seed);
        inner
    }
}

impl From<&Seed> for WalletSeed {
    fn from(seed: &Seed) -> Self {
        seed.0.clone()
    }
}

impl FromStr for Seed {
    type Err = SeedError;

    /// Tries hex, then mnemonic, in that order. The match upstream's
    /// `WalletSeed::FromStr` does on the raw seed type — same input strings
    /// produce equivalent seeds via either constructor.
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
    /// Zeroize the seed bytes on drop. `WalletSeed` itself doesn't implement
    /// `Zeroize`, so we reach into each variant's array and clear it
    /// in-place. The enum tag stays as-is; only the entropy is wiped.
    fn drop(&mut self) {
        match &mut self.0 {
            WalletSeed::Short(bytes) => bytes.zeroize(),
            WalletSeed::Medium(bytes) => bytes.zeroize(),
            WalletSeed::Long(bytes) => bytes.zeroize(),
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
        Role::Metadata => 4,
    }
}

// ---------------------------------------------------------------------------
// Mnemonic utilities
// ---------------------------------------------------------------------------

/// Mnemonic phrase generation, validation, and word-list conversion.
///
/// [`generate`] defaults to 256 bits of entropy (24 words), [`validate`]
/// runs the BIP-39 checksum, and [`words`] / [`join`] split / re-join the
/// phrase on whitespace.
pub mod mnemonic {
    use super::SeedError;

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
    /// validate the phrase — that's [`validate`]'s job.
    pub fn words(phrase: &str) -> Vec<&str> {
        phrase.split_whitespace().collect()
    }

    /// Re-join a word vector into a single phrase string with single spaces.
    pub fn join(words: &[String]) -> String {
        words.join(" ")
    }

    /// Convenience constructor used by tests: parse a phrase and surface a
    /// strongly-typed parse error rather than the bool from [`validate`].
    #[allow(dead_code)]
    pub(crate) fn parse(phrase: &str) -> Result<bip39::Mnemonic, SeedError> {
        bip39::Mnemonic::parse(phrase).map_err(SeedError::InvalidMnemonic)
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
    /// (`ledger/helpers/src/versions/common/types.rs` —
    /// `should_decode_wallet_seeds_in_different_formats`).
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon diesel";
    const TEST_SEED_HEX: &str = "a51c86de32d0791f7cffc3bdff1abd9bb54987f0ed5effc30c936dddbb9afd9d530c8db445e4f2d3ea42a321b260e022aadf05987c9a67ec7b6b6ca1d0593ec9";

    #[test]
    fn from_mnemonic_matches_upstream_test_vector() {
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        assert_eq!(hex::encode(seed.as_bytes()), TEST_SEED_HEX);
    }

    #[test]
    fn from_mnemonic_matches_from_hex_for_same_seed() {
        let from_mnemonic = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        let from_hex = Seed::from_hex(TEST_SEED_HEX).unwrap();
        assert_eq!(from_mnemonic.as_bytes(), from_hex.as_bytes());
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

    #[test]
    fn into_wallet_seed_round_trips() {
        let seed = Seed::from_hex(TEST_SEED_HEX).unwrap();
        let bytes_before = seed.as_bytes().to_vec();
        let ws: WalletSeed = seed.into();
        assert_eq!(ws.as_bytes(), bytes_before.as_slice());
    }

    #[test]
    fn derive_at_produces_32_bytes_per_role() {
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        for role in [
            Role::UnshieldedExternal,
            Role::UnshieldedInternal,
            Role::Dust,
            Role::Zswap,
            Role::Metadata,
        ] {
            let key = seed.account(0).role(role.clone()).derive_at(0).unwrap();
            assert_eq!(key.len(), 32, "{role:?} derivation must be 32 bytes");
        }
    }

    #[test]
    fn derive_at_differs_per_role() {
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        let zswap = seed.account(0).role(Role::Zswap).derive_at(0).unwrap();
        let dust = seed.account(0).role(Role::Dust).derive_at(0).unwrap();
        let unshielded_ext = seed
            .account(0)
            .role(Role::UnshieldedExternal)
            .derive_at(0)
            .unwrap();
        assert_ne!(zswap, dust);
        assert_ne!(zswap, unshielded_ext);
        assert_ne!(dust, unshielded_ext);
    }

    #[test]
    fn derive_at_differs_per_account_and_index() {
        let seed = Seed::from_mnemonic(TEST_MNEMONIC).unwrap();
        let acct0_idx0 = seed.account(0).role(Role::Zswap).derive_at(0).unwrap();
        let acct1_idx0 = seed.account(1).role(Role::Zswap).derive_at(0).unwrap();
        let acct0_idx1 = seed.account(0).role(Role::Zswap).derive_at(1).unwrap();
        assert_ne!(acct0_idx0, acct1_idx0, "account must change the key");
        assert_ne!(acct0_idx0, acct0_idx1, "index must change the key");
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

    #[test]
    fn address_derivation_matches_walletseed_path() {
        // The same underlying seed bytes — once via Seed, once via the raw
        // upstream WalletSeed — must produce identical bech32 addresses.
        // This is the contract that lets callers swap construction styles
        // without changing their on-chain identity.
        use crate::Network;
        use crate::address::{derive_shielded, derive_unshielded};

        let seed = Seed::from_hex(TEST_SEED_HEX).unwrap();
        let ws = WalletSeed::try_from_hex_str(TEST_SEED_HEX).unwrap();

        assert_eq!(
            seed.unshielded_address(Network::Preprod),
            derive_unshielded(&ws, Network::Preprod),
        );
        assert_eq!(
            seed.shielded_address(Network::Preprod),
            derive_shielded(&ws, Network::Preprod),
        );
    }
}
