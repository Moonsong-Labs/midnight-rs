//! The token imbalance of a proven transaction.

use std::collections::BTreeMap;

use crate::{LedgerVersion, TokenType, WalletError};

/// The imbalance of each token at each segment of a proven transaction.
///
/// Each entry maps a token and a segment to what the segment puts in of the
/// token, less what it takes out. A negative value is a deficit, which a
/// payer covers from its own coins. A positive value is a surplus, which a
/// payer takes as an output. A token that a segment moves and balances reads
/// 0, and a token that it does not move has no entry. The Dust entry, at
/// segment 0, is what the transaction's Dust spends and Dust registrations
/// offer for its fee, and the fee itself is not in it.
///
/// `MidnightProvider::balance_transaction_with_tokens` in `midnight-provider`
/// pays every deficit that a transaction carries. A payer reads this first,
/// to check that the transaction asks for no more than the payer agreed to.
///
/// # Errors
///
/// [`WalletError::UnknownLedger`] when the bytes are no transaction of a
/// linked generation, and [`WalletError::Transfer`] when they do not
/// deserialize or a balance overflows.
pub fn imbalances(tx_bytes: &[u8]) -> Result<BTreeMap<(TokenType, u16), i128>, WalletError> {
    match LedgerVersion::of_transaction(tx_bytes)? {
        LedgerVersion::V8 => crate::ledger_8::imbalances(tx_bytes),
        LedgerVersion::V9 => crate::ledger_9::imbalances(tx_bytes),
    }
}
