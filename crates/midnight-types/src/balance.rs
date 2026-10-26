//! What a wallet holds, as owned values: the three balance legs and the
//! spendable shielded coins.

use std::fmt;

use crate::{NIGHT, Nullifier, ShieldedTokenType, UnshieldedTokenType};

/// The wallet's Dust, valued at the block time that its next build uses for
/// Dust.
///
/// A sync or resync sets that time to one second after the last Dust event it
/// replayed. A resync that replays no new Dust event keeps the time it had.
/// The chain time replaces a time that is too old for the chain to accept as a
/// Dust anchor, and a sync that replays no Dust event uses the chain time too.
/// So the readings can lag the chain tip by as much as the Dust grace period,
/// and they leave out the Dust that accrued after that time.
///
/// A build reserves the Dust UTXOs it spends. The reservation ends when a sync
/// or resync sees the chain confirm the transaction. It also ends when a
/// release hands the UTXOs back, or when its TTL ends.
/// [`Self::spendable_utxos`] and [`Self::spendable_speck`] leave the reserved
/// UTXOs out, and [`Self::balance_speck`] counts them.
#[derive(Debug, Clone, Default)]
pub struct DustBalance {
    /// How many Dust UTXOs no pending build reserves.
    pub spendable_utxos: usize,
    /// All the Dust, reserved or not, in SPECK (1 DUST = 10^15 SPECK).
    pub balance_speck: u128,
    /// The Dust a new build can draw on now, in SPECK.
    ///
    /// A wallet whose only Dust UTXO a pending build reserves reads 0 here and
    /// a positive [`Self::balance_speck`].
    pub spendable_speck: u128,
    /// Whether any tNIGHT this wallet holds generates dust.
    ///
    /// Derived from the coins on hand, which is all the indexer reports, so it
    /// is not the same as "the address is registered". A wallet that registers
    /// and then spends every tNIGHT reads `false` here while its registration
    /// still stands, and tNIGHT arriving later still generates. Use
    /// [`Self::unregistered_night_utxos`] to decide whether there is anything
    /// left to register.
    pub night_generates_dust: bool,
    /// How many tNIGHT UTXOs still generate nothing, which is how many more
    /// `register_dust` calls this wallet needs. A registration covers the one
    /// UTXO it spends and every coin arriving afterwards, so this counts down
    /// rather than clearing at once.
    pub unregistered_night_utxos: usize,
}

#[derive(Debug, Clone)]
pub struct UnshieldedUtxoInfo {
    /// The UTXO's typed token id. Use [`token_type_hex`](Self::token_type_hex)
    /// for display / log output.
    pub token_type: UnshieldedTokenType,
    pub value: u128,
    /// Whether a new build can spend this UTXO. It is `false` while a pending
    /// build reserves the UTXO.
    pub spendable: bool,
}

impl UnshieldedUtxoInfo {
    /// The token id as [`UnshieldedTokenType`]'s `Display` writes it, for
    /// human-readable logs / debug output.
    pub fn token_type_hex(&self) -> String {
        self.token_type.to_string()
    }
}

impl fmt::Display for UnshieldedUtxoInfo {
    /// Render as `NIGHT: <value>` when the token is the native unshielded
    /// asset, otherwise `<8-char-hex-prefix>…: <value>`. The "t"-prefixed
    /// testnet name is a network convention the SDK can't infer from the
    /// token id alone — callers running against testnet can format manually
    /// using [`token_type_hex`](Self::token_type_hex) and [`value`](Self::value).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.token_type == NIGHT {
            write!(f, "NIGHT: {}", self.value)
        } else {
            write!(f, "{:.8}…: {}", self.token_type, self.value)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShieldedCoinBalance {
    /// The coin's typed token id. Use [`token_type_hex`](Self::token_type_hex)
    /// for display / log output. Treat shielded token ids as opaque — the
    /// zero id is **not** NIGHT; see `docs/tokens.md`.
    pub token_type: ShieldedTokenType,
    pub value: u128,
    /// Whether a new build can spend this coin. It is `false` while a pending
    /// build reserves the coin.
    pub spendable: bool,
}

impl ShieldedCoinBalance {
    /// The token id as [`ShieldedTokenType`]'s `Display` writes it, for
    /// human-readable logs / debug output.
    pub fn token_type_hex(&self) -> String {
        self.token_type.to_string()
    }
}

impl fmt::Display for ShieldedCoinBalance {
    /// Render as `<8-char-hex-prefix>…: <value>`. There is no shielded NIGHT;
    /// all shielded token ids are treated as opaque.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}…: {}", self.token_type, self.value)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ShieldedBalance {
    pub coins: Vec<ShieldedCoinBalance>,
    pub total_count: usize,
}

/// A single spendable shielded coin, addressed by its full coin info.
///
/// Unlike [`ShieldedCoinBalance`], which carries no nonce, this names one
/// concrete coin: the `nonce`, `token_type`, and `value`
/// are exactly the `ShieldedCoinInfo { nonce, color, value }` a circuit argument
/// needs, and `nullifier` pins this exact coin when it is spent as a shielded
/// input. A circuit like `receiveShielded(coin)` re-commits the coin's exact
/// `nonce`/`color`/`value`, so the caller must both name the precise coin and
/// spend that same one — amount-based selection cannot express that, and this
/// accessor can. Enumerate with `WalletFacade::spendable_shielded_coins`.
#[derive(Debug, Clone)]
pub struct SpendableShieldedCoin {
    /// The coin's typed token id (its "color"). Treat shielded token ids as
    /// opaque; see [`ShieldedCoinBalance::token_type`].
    pub token_type: ShieldedTokenType,
    pub value: u128,
    /// The coin's 32-byte nonce, needed to build a `ShieldedCoinInfo` circuit
    /// argument that re-commits this exact coin.
    pub nonce: [u8; 32],
    /// Pins this exact coin when it is selected as a shielded input, so the SDK
    /// spends this coin and not another of the same token type / value.
    pub nullifier: Nullifier,
}

impl SpendableShieldedCoin {
    /// The token id as [`ShieldedTokenType`]'s `Display` writes it, for logs /
    /// debug output.
    pub fn token_type_hex(&self) -> String {
        self.token_type.to_string()
    }
}

#[derive(Debug, Clone, Default)]
pub struct WalletBalance {
    pub dust: DustBalance,
    pub unshielded: Vec<UnshieldedUtxoInfo>,
    pub shielded: ShieldedBalance,
}

impl WalletBalance {
    /// The total value of every coin of `token` in [`Self::shielded`].
    ///
    /// The value is in the token's smallest unit. The sum counts every coin of
    /// `token`, including the coins that a pending build reserves.
    /// [`Self::spendable_shielded_value`] leaves those out. It is 0 when the
    /// wallet holds no coin of `token`, and it saturates at `u128::MAX`.
    pub fn shielded_value(&self, token: ShieldedTokenType) -> u128 {
        saturating_sum(
            self.shielded
                .coins
                .iter()
                .filter(|coin| coin.token_type == token)
                .map(|coin| coin.value),
        )
    }

    /// The value of the coins of `token` in [`Self::shielded`] that a new
    /// build can spend.
    ///
    /// It is [`Self::shielded_value`] without the coins that a pending build
    /// reserves.
    pub fn spendable_shielded_value(&self, token: ShieldedTokenType) -> u128 {
        saturating_sum(
            self.shielded
                .coins
                .iter()
                .filter(|coin| coin.token_type == token && coin.spendable)
                .map(|coin| coin.value),
        )
    }

    /// The total value of every UTXO of `token` in [`Self::unshielded`].
    ///
    /// The value is in the token's smallest unit, which is STAR for [`NIGHT`].
    /// The sum counts every UTXO of `token`, including the UTXOs that a pending
    /// build reserves. [`Self::spendable_unshielded_value`] leaves those out.
    /// It is 0 when the wallet holds no UTXO of `token`, and it saturates at
    /// `u128::MAX`.
    pub fn unshielded_value(&self, token: UnshieldedTokenType) -> u128 {
        saturating_sum(
            self.unshielded
                .iter()
                .filter(|utxo| utxo.token_type == token)
                .map(|utxo| utxo.value),
        )
    }

    /// The value of the UTXOs of `token` in [`Self::unshielded`] that a new
    /// build can spend.
    ///
    /// It is [`Self::unshielded_value`] without the UTXOs that a pending build
    /// reserves.
    pub fn spendable_unshielded_value(&self, token: UnshieldedTokenType) -> u128 {
        saturating_sum(
            self.unshielded
                .iter()
                .filter(|utxo| utxo.token_type == token && utxo.spendable)
                .map(|utxo| utxo.value),
        )
    }
}

fn saturating_sum(values: impl Iterator<Item = u128>) -> u128 {
    values.fold(0, u128::saturating_add)
}

#[cfg(test)]
mod tests {
    use midnight_helpers::HashOutput;

    use super::{
        ShieldedBalance, ShieldedCoinBalance, ShieldedTokenType, UnshieldedTokenType,
        UnshieldedUtxoInfo, WalletBalance,
    };

    fn balance(entries: &[(u8, u128, bool)]) -> WalletBalance {
        let coins: Vec<_> = entries
            .iter()
            .map(|&(id, value, spendable)| ShieldedCoinBalance {
                token_type: ShieldedTokenType(HashOutput([id; 32])),
                value,
                spendable,
            })
            .collect();
        WalletBalance {
            unshielded: entries
                .iter()
                .map(|&(id, value, spendable)| UnshieldedUtxoInfo {
                    token_type: UnshieldedTokenType(HashOutput([id; 32])),
                    value,
                    spendable,
                })
                .collect(),
            shielded: ShieldedBalance {
                total_count: coins.len(),
                coins,
            },
            ..WalletBalance::default()
        }
    }

    /// Each leg holds one entry per coin, so a sum that skips the filter, or
    /// takes only the first or the last match, gives another number.
    #[test]
    fn a_token_value_sums_every_entry_of_that_token_only() {
        let balance = balance(&[(1, 5, true), (2, 100, true), (1, 7, true)]);
        let token = HashOutput([1; 32]);
        assert_eq!(balance.shielded_value(ShieldedTokenType(token)), 12);
        assert_eq!(balance.unshielded_value(UnshieldedTokenType(token)), 12);
    }

    /// The sums have no error channel, so an overflow must neither panic nor
    /// wrap.
    #[test]
    fn a_token_value_saturates_on_overflow() {
        let balance = balance(&[(1, u128::MAX, true), (1, 1, true)]);
        let token = HashOutput([1; 32]);
        assert_eq!(balance.shielded_value(ShieldedTokenType(token)), u128::MAX);
        assert_eq!(
            balance.unshielded_value(UnshieldedTokenType(token)),
            u128::MAX
        );
    }

    /// A spendable sum that skips the flag reads the total, and one that
    /// skips the token filter counts the other token too.
    #[test]
    fn a_spendable_value_sums_the_spendable_entries_of_that_token_only() {
        let balance = balance(&[(1, 5, true), (1, 7, false), (2, 100, true)]);
        let token = HashOutput([1; 32]);
        assert_eq!(
            balance.spendable_shielded_value(ShieldedTokenType(token)),
            5
        );
        assert_eq!(
            balance.spendable_unshielded_value(UnshieldedTokenType(token)),
            5
        );
    }
}
