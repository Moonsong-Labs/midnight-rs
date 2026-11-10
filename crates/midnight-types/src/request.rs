//! [`TransactionRequest`]: what a `MidnightProvider` builds.

use std::time::Duration;

use crate::build::ContractPart;
use crate::coin::{
    DustPublicKey, ShieldedRecipient, ShieldedTokenType, UnshieldedRecipient, UnshieldedTokenType,
};

/// What to build. Make it with a constructor, then adjust it with a modifier.
///
/// `MidnightProvider::send_transaction` fills, funds, proves, signs and sends
/// the transaction that a request names. `MidnightProvider::fill` stops before
/// the send.
///
/// A request is not `Clone`. Make a new one to build again.
#[derive(Debug)]
pub struct TransactionRequest {
    pub(crate) kind: RequestKind,
    pub(crate) pays_fee: bool,
    pub(crate) ttl: Option<Duration>,
    pub(crate) coin_selection: CoinSelection,
}

/// The order in which a wallet draws the coins and UTXOs of a transfer.
///
/// [`TransactionRequest::with_coin_selection`] sets it for one request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// TODO: replace both orders with best-fit selection,
// <https://github.com/Moonsong-Labs/midnight-rs/issues/145>
pub enum CoinSelection {
    /// The largest coins first. It spends the fewest inputs. The default.
    #[default]
    LargestFirst,
    /// The smallest coins first. It consolidates small coins.
    SmallestFirst,
}

/// The transaction that a [`TransactionRequest`] names.
#[derive(Debug)]
pub(crate) enum RequestKind {
    Shielded {
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: ShieldedRecipient,
    },
    Unshielded {
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: UnshieldedRecipient,
    },
    ShieldedSwap {
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    },
    RegisterDust {
        /// `None` names this wallet's own Dust public key.
        target: Option<DustPublicKey>,
    },
    Sponsor(Sponsored),
    Contract(ContractPart),
}

/// The tagged bytes of a proven transaction that a sponsor pays the fee of.
pub(crate) struct Sponsored(pub(crate) Vec<u8>);

impl std::fmt::Debug for Sponsored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} bytes", self.0.len())
    }
}

impl TransactionRequest {
    fn paid(kind: RequestKind) -> Self {
        Self {
            kind,
            pays_fee: true,
            ttl: None,
            coin_selection: CoinSelection::default(),
        }
    }

    /// Send `amount` of `token` to `to`. Change returns to this wallet.
    ///
    /// `amount` may span several coins. The wallet selects the coins up
    /// front, in the order of [`Self::with_coin_selection`], and the remainder
    /// returns to it as a change output. So a payment is bounded by the
    /// wallet's total balance in `token`, not by its largest coin. A transfer
    /// of the whole balance to this wallet's own address joins it into one
    /// coin.
    ///
    /// Read `to` from a shielded address with
    /// [`parse_shielded_recipient`](crate::address::parse_shielded_recipient),
    /// or derive it from a seed with
    /// [`Seed::shielded_recipient`](crate::hd::Seed::shielded_recipient).
    pub fn shielded(token: ShieldedTokenType, amount: u128, to: ShieldedRecipient) -> Self {
        Self::paid(RequestKind::Shielded {
            token_type: token,
            amount,
            recipient: to,
        })
    }

    /// Send `amount` of `token` to `to`. Change returns to this wallet.
    ///
    /// Read `to` from an unshielded address with
    /// [`parse_unshielded_recipient`](crate::address::parse_unshielded_recipient),
    /// or derive it from a seed with
    /// [`Seed::unshielded_recipient`](crate::hd::Seed::unshielded_recipient).
    pub fn unshielded(token: UnshieldedTokenType, amount: u128, to: UnshieldedRecipient) -> Self {
        Self::paid(RequestKind::Unshielded {
            token_type: token,
            amount,
            recipient: to,
        })
    }

    /// One half of a two-party swap: give `give_amount` of `give`, receive
    /// `receive_amount` of `receive`. It pays no fee.
    ///
    /// The half spends `give_amount` of `give` and creates an output for
    /// `receive_amount` of `receive` that pays this wallet. It is unbalanced
    /// (`+give`, `-receive`), so it cannot pay its own fee.
    ///
    /// The counterparty fills the exact mirror,
    /// `shielded_swap(receive, receive_amount, give, give_amount)`. A sponsor,
    /// either party or a third one, merges the two halves into a balanced,
    /// fee-less transaction with `merge_transactions`, and sends it with
    /// [`Self::sponsor`]. The two halves must carry exactly mirrored
    /// `(token, amount)` pairs, or the merge does not balance.
    pub fn shielded_swap(
        give: ShieldedTokenType,
        give_amount: u128,
        receive: ShieldedTokenType,
        receive_amount: u128,
    ) -> Self {
        Self {
            kind: RequestKind::ShieldedSwap {
                give_token: give,
                give_amount,
                receive_token: receive,
                receive_amount,
            },
            pays_fee: false,
            ttl: None,
            coin_selection: CoinSelection::default(),
        }
    }

    /// Register one tNIGHT UTXO of this wallet for Dust to this wallet's Dust
    /// key.
    ///
    /// The registration pays its own fee from the Dust that the UTXO can
    /// generate, so a wallet needs no Dust to register. The wallet picks the
    /// unregistered UTXO that carries the most of that Dust, and the fill
    /// fails when every tNIGHT UTXO is registered. Send one until
    /// [`DustBalance::unregistered_night_utxos`](crate::DustBalance::unregistered_night_utxos)
    /// is 0. A UTXO whose creation time the indexer did not report counts as
    /// one hour old.
    pub fn register_dust() -> Self {
        Self::paid(RequestKind::RegisterDust { target: None })
    }

    /// Register one tNIGHT UTXO of this wallet for Dust to `target`, another
    /// wallet's Dust public key. See [`Self::register_dust`].
    ///
    /// That wallet can then pay the fees of this wallet's transactions. Read
    /// `target` from the Dust address of that wallet with
    /// [`parse_dust_address`](crate::address::parse_dust_address).
    ///
    /// The registration sets the Dust public key of the whole NIGHT address.
    /// A later registration replaces it, so a [`Self::register_dust`] points
    /// the NIGHT address back to this wallet's own key. tNIGHT that reaches
    /// the address generates Dust for the key that the last registration set.
    /// tNIGHT that already generates Dust keeps its key until the wallet
    /// spends it.
    pub fn register_dust_to(target: DustPublicKey) -> Self {
        Self::paid(RequestKind::RegisterDust {
            target: Some(target),
        })
    }

    /// Pay the Dust fee of the proven transaction `tx`, which another party or
    /// a merge made.
    ///
    /// `tx` is the tagged encoding of a proven transaction, such as the bytes
    /// of `TxEnvelope::into_bytes`. The fill adds a part of this wallet's own
    /// that pays the fee, and leaves `tx` and its proofs as they are.
    ///
    /// It covers the fee only. `tx` must balance every other token, and the
    /// fill refuses a deficit of one. A `tx` that already pays its own fee
    /// comes back unchanged, with nothing reserved.
    ///
    /// The Dust that the part spends stays reserved until the earliest TTL of
    /// the intents of `tx`. When `tx` has no intent, it stays reserved for the
    /// chain's `global_ttl` after the chain time.
    pub fn sponsor(tx: Vec<u8>) -> Self {
        Self::paid(RequestKind::Sponsor(Sponsored(tx)))
    }

    /// The transaction of a contract deploy, call or maintenance update.
    /// Only midnight-contract makes a [`ContractPart`].
    #[doc(hidden)]
    pub fn contract(part: ContractPart) -> Self {
        Self::paid(RequestKind::Contract(part))
    }

    /// Leave the Dust fee for another wallet to pay.
    ///
    /// The transaction is not valid on its own, so a send refuses it. Read
    /// its bytes with `TxEnvelope::into_bytes`. The payer completes it with
    /// [`Self::sponsor`], alone or after a merge with other transactions.
    ///
    /// A Dust registration and a sponsor pay a fee by what they are, so their
    /// fill fails with [`WalletError::Transfer`](crate::WalletError::Transfer).
    pub fn without_fee(mut self) -> Self {
        self.pays_fee = false;
        self
    }

    /// Keep the transaction valid for `ttl` after the chain time of the fill.
    ///
    /// The default is the chain's `global_ttl`, the longest TTL the chain
    /// accepts. A shorter TTL bounds how long a transaction that never
    /// confirms can still land, and how long the wallet holds its inputs.
    /// Once the TTL passes, the transaction can no longer land, and the next
    /// fill frees the inputs it reserved. So after a send whose fate is
    /// unknown, such as `SubmitError::Dropped`, a fill from the same inputs is
    /// safe from a double spend after `ttl`, not after the whole `global_ttl`.
    ///
    /// The fill fails with
    /// [`WalletError::TtlOutOfRange`](crate::WalletError::TtlOutOfRange) when
    /// `ttl` is under one second or longer than `global_ttl`. It fails with
    /// [`WalletError::TtlWithoutIntent`](crate::WalletError::TtlWithoutIntent)
    /// for a fee-less shielded transfer or a swap half, which carry no intent
    /// to hold a TTL. A sponsor keeps the TTL of the transaction that it pays
    /// for, so its fill fails with
    /// [`WalletError::Transfer`](crate::WalletError::Transfer).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Draw the coins and UTXOs of the transfer in the order of `selection`.
    ///
    /// [`CoinSelection::LargestFirst`], the default, spends the fewest inputs.
    /// Each shielded input adds a proof and a fee.
    /// [`CoinSelection::SmallestFirst`] spends the small coins that the
    /// default never spends. Nothing caps how many inputs it draws, so use it
    /// for a deliberate consolidation, not for an ordinary payment.
    ///
    /// The order applies to the coins of a shielded transfer and of a swap
    /// half, and to the UTXOs of an unshielded transfer. Two draws do not read
    /// it: a Dust registration picks its tNIGHT UTXO by the Dust that the UTXO
    /// can generate, and the fee draws on the wallet's Dust.
    pub fn with_coin_selection(mut self, selection: CoinSelection) -> Self {
        self.coin_selection = selection;
        self
    }
}
