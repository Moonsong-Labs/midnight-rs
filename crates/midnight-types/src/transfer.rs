//! The vocabulary of a transfer build: the request that names one
//! ([`TransferRequest`]), the payments it makes ([`Payment`]), what it spends
//! ([`SpentInputs`]), and its result ([`TransferResult`]).
//!
//! The builds themselves are per ledger generation, in
//! [`crate::ledger_8`] and [`crate::ledger_9`].

use std::collections::BTreeMap;

use midnight_helpers::{CoinSelectionStrategy, HashOutput, Timestamp};

use crate::coin::{DustNullifier, Nullifier, ShieldedTokenType, UnshieldedTokenType};
use crate::error::WalletError;
use crate::ledger_version::LedgerVersion;

/// A proven transaction a build made, and the inputs the build reserved for
/// it.
pub struct TransferResult {
    /// The proven transaction, tagged with its generation's encoding.
    pub tx_bytes: Vec<u8>,
    /// The generation the transaction is built for.
    pub ledger_version: LedgerVersion,
    /// The inputs of this transaction that the builds of this wallet
    /// reserved. `WalletFacade::release` takes them back if the transaction
    /// never reaches the chain. A transaction that pays the fee of another
    /// party's transaction also spends that party's inputs, and this names
    /// only the Dust of the fee.
    pub spent: SpentInputs,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    ///
    /// The `false` skips the time-to-dismiss check, and the node applies that
    /// check at submit. It rejects a transaction, as
    /// `FeeCalculation(OutsideTimeToDismiss)`, when validating the whole
    /// transaction and applying its guaranteed part take more time than its
    /// size allows. The allowance is `time_to_dismiss_per_byte` per byte, and
    /// never less than `min_time_to_dismiss`. The check never changes the
    /// fee, but a quote does not mean that the node accepts the transaction.
    pub fee_speck: u128,
}

impl std::fmt::Debug for TransferResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferResult")
            .field("tx_bytes", &format_args!("{} bytes", self.tx_bytes.len()))
            .field("ledger_version", &self.ledger_version)
            .field("spent", &self.spent)
            .field("fee_speck", &self.fee_speck)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    /// The intent hash of the transaction that created the UTXO.
    pub intent_hash: HashOutput,
    /// The number of the UTXO among that transaction's outputs.
    pub output_index: u32,
}

/// What one build spends, in the form a reservation takes.
///
/// A build reserves its inputs so a later build in the same process does not
/// re-select them before the indexer surfaces the spend. Reserve them, and
/// hand them back if the build will never reach the chain.
///
/// Each input carries the stamp of its reservation, because the stamp is half
/// of a reservation's identity. A release names both, so a build hands back
/// its own entry, not one a later build made over the same input in a later
/// block. Two reservations in one block share the stamp, which is why a
/// reservation is released once.
///
/// One transaction can hold the inputs of two reservations with different
/// stamps. A contract call or a deploy with a shielded offer reserves its
/// pinned coins in its own build context, and the Dust for its fee in a fresh
/// one. [`Self::merge`] joins the two, so one value hands both back.
#[derive(Debug, Default, Clone)]
pub struct SpentInputs {
    /// The Dust spends that fund the fee, by nullifier.
    pub dust: Vec<ReservedInput<DustNullifier>>,
    /// The unshielded UTXOs the build spends.
    pub unshielded: Vec<ReservedInput<SpentUtxoKey>>,
    /// The shielded coins the build spends, by nullifier.
    pub shielded: Vec<ReservedInput<Nullifier>>,
}

/// One input that a build reserved, and the stamp of that reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedInput<T> {
    /// The input: a nullifier, or the key of an unshielded UTXO.
    pub input: T,
    /// The chain time the build read when it selected the input, which is
    /// what the reservation is stamped with.
    pub reserved_at: Timestamp,
}

impl SpentInputs {
    /// The shielded coins a build pinned, for one that spends nothing else.
    pub fn from_shielded(shielded: Vec<Nullifier>, reserved_at: Timestamp) -> Self {
        Self {
            shielded: shielded
                .into_iter()
                .map(|input| ReservedInput { input, reserved_at })
                .collect(),
            ..Self::default()
        }
    }

    /// Add the inputs of `other`. Each input keeps its own stamp.
    pub fn merge(&mut self, other: SpentInputs) {
        self.dust.extend(other.dust);
        self.unshielded.extend(other.unshielded);
        self.shielded.extend(other.shielded);
    }

    /// Whether this build drew nothing at all, so there is nothing to reserve
    /// and nothing to hand back.
    pub fn is_empty(&self) -> bool {
        self.dust.is_empty() && self.unshielded.is_empty() && self.shielded.is_empty()
    }
}

/// One payment of a transfer: `amount` of `token_type` to `recipient`.
///
/// `T` is [`ShieldedTokenType`] for a shielded transfer and
/// [`UnshieldedTokenType`] for an unshielded one, so a payment cannot go to a
/// transfer of the other kind.
///
/// ```
/// use midnight_types::{NIGHT, Payment};
///
/// let payment = Payment::new(NIGHT, 5, "mn_addr_undeployed1...");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment<T> {
    /// The token the payment sends.
    pub token_type: T,
    /// The amount, in the token's smallest unit. A build refuses 0.
    pub amount: u128,
    /// The bech32 address of the recipient: `mn_shield-addr_*` for a shielded
    /// payment, `mn_addr_*` for an unshielded one.
    pub recipient: String,
}

impl<T> Payment<T> {
    /// A payment of `amount` of `token_type` to the bech32 address
    /// `recipient`.
    pub fn new(token_type: T, amount: u128, recipient: impl Into<String>) -> Self {
        Self {
            token_type,
            amount,
            recipient: recipient.into(),
        }
    }
}

/// The sum that `payments` send of each token, which is what a build selects
/// inputs for and returns change from.
///
/// # Errors
///
/// [`WalletError::Transfer`] when `payments` is empty, when a payment sends 0,
/// or when the payments of one token add up to more than `u128::MAX`.
pub(crate) fn totals_per_token<T: Copy + Ord + std::fmt::Display>(
    payments: &[Payment<T>],
) -> Result<BTreeMap<T, u128>, WalletError> {
    if payments.is_empty() {
        return Err(WalletError::Transfer(
            "a transfer needs at least one payment".into(),
        ));
    }
    let mut totals = BTreeMap::new();
    for (index, payment) in payments.iter().enumerate() {
        if payment.amount == 0 {
            return Err(WalletError::Transfer(format!(
                "payment {index} sends 0; every payment must send more than 0"
            )));
        }
        let total = totals.entry(payment.token_type).or_insert(0u128);
        *total = total.checked_add(payment.amount).ok_or_else(|| {
            WalletError::Transfer(format!(
                "the payments of token {} add up to more than u128::MAX",
                payment.token_type
            ))
        })?;
    }
    Ok(totals)
}

/// One transfer a build can make.
///
/// Data rather than a closure over the builder, so a caller can hand a build
/// to a wallet it reaches only through a trait.
#[derive(Debug)]
pub enum TransferKind {
    /// A shielded (Zswap) transfer that pays each of `payments`. See
    /// `TransferBuilder::prepare_shielded`.
    Shielded {
        payments: Vec<Payment<ShieldedTokenType>>,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// An unshielded (UTXO) transfer that pays each of `payments`. See
    /// `TransferBuilder::prepare_unshielded`.
    Unshielded {
        payments: Vec<Payment<UnshieldedTokenType>>,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// One half of a two-party shielded swap, always fee-less. See
    /// `TransferBuilder::prepare_shielded_swap`.
    ShieldedSwap {
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    },
    /// A dust-address registration. See
    /// `TransferBuilder::prepare_register_dust`.
    DustRegistration {
        /// Fallback creation time for the UTXOs whose own creation time the
        /// indexer did not report.
        utxo_ctime: Option<u64>,
    },
}

/// One build, as data: what to make, and how to choose the inputs that make
/// it.
#[derive(Debug)]
pub struct TransferRequest {
    /// The transfer to build.
    pub kind: TransferKind,
    /// How to order the coins and UTXOs the build draws on.
    pub coin_selection: CoinSelectionStrategy,
}

impl TransferRequest {
    /// A request that orders its inputs the default way.
    pub fn new(kind: TransferKind) -> Self {
        Self {
            kind,
            coin_selection: CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on. See
    /// `TransferBuilder::with_coin_selection`.
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }
}

impl From<TransferKind> for TransferRequest {
    fn from(kind: TransferKind) -> Self {
        Self::new(kind)
    }
}

/// Recover a printable message from a caught panic payload.
///
/// Shared with the provider's fee-paying path, which proves through the same
/// backend and needs the same treatment.
pub fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    "proof backend panicked with a non-string payload".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_message_handles_both_payload_shapes() {
        assert_eq!(
            panic_message(Box::new("static str".to_string())),
            "static str"
        );
        assert_eq!(panic_message(Box::new("literal")), "literal");
        assert!(panic_message(Box::new(42u8)).contains("non-string payload"));
    }

    const A: UnshieldedTokenType = UnshieldedTokenType(HashOutput([1; 32]));
    const B: UnshieldedTokenType = UnshieldedTokenType(HashOutput([2; 32]));

    fn pay(token_type: UnshieldedTokenType, amount: u128) -> Payment<UnshieldedTokenType> {
        Payment::new(token_type, amount, "recipient")
    }

    /// A build selects inputs once per token and returns one change output per
    /// token, so payments of one token must add up, and other tokens must not
    /// join their sum.
    #[test]
    fn the_payments_of_one_token_add_up_to_one_total() {
        let totals = totals_per_token(&[pay(A, 3), pay(B, 5), pay(A, 4)]).unwrap();
        assert_eq!(totals, BTreeMap::from([(A, 7), (B, 5)]));
    }

    /// Each input passes every other check, so only the refusal it names can
    /// turn it into an error. A zero payment beside a valid one is refused, not
    /// dropped: the ledger refuses a zero-value UTXO output at submit.
    #[test]
    fn a_transfer_refuses_no_payments_a_zero_payment_and_an_overflowing_sum() {
        for (case, payments) in [
            ("no payments", vec![]),
            ("a zero payment", vec![pay(A, 3), pay(B, 0)]),
            ("an overflowing sum", vec![pay(A, u128::MAX), pay(A, 1)]),
        ] {
            assert!(
                matches!(totals_per_token(&payments), Err(WalletError::Transfer(_))),
                "{case}"
            );
        }
    }

    #[test]
    fn the_default_coin_selection_is_largest_first() {
        assert_eq!(
            TransferRequest::new(TransferKind::DustRegistration { utxo_ctime: None })
                .coin_selection,
            CoinSelectionStrategy::LargestFirst,
            "every shielded input carries its own proof, so the default \
             spends the fewest inputs"
        );
    }
}
