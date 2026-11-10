//! The stages of a transaction between a `MidnightProvider` and the wallet
//! that it holds: filled, funded, proven and signed.

use crate::build::{Fill, Funded, Reservation};

pub(crate) type FinalizedTx = midnight_helpers::FinalizedTransaction<midnight_helpers::DefaultDB>;

/// A transaction with its outputs, TTL and chain parameters, which a wallet
/// funds next.
///
/// A `MidnightProvider` fills it, and
/// [`MidnightWallet::fund_transaction`](crate::MidnightWallet::fund_transaction)
/// takes it.
pub struct FilledTx(pub(crate) Fill);

/// A transaction that a wallet funded, not yet proven.
///
/// It holds the wallet's inputs, change and Dust fee, and the reservation of
/// what they spend. A drop hands those inputs back to the wallet.
pub struct FundedTx {
    pub(crate) funded: Funded,
    pub(crate) reservation: Reservation,
}

/// A proven and sealed transaction, which the wallet signs next.
///
/// A drop hands the inputs that its wallet reserved back to the wallet.
pub struct ProvenTx {
    pub(crate) tx: FinalizedTx,
    pub(crate) reservation: Reservation,
}

/// A signed transaction, ready to send.
///
/// A drop before the send hands the inputs that its wallet reserved back to
/// the wallet. A transaction that pays no fee, from a request made
/// `without_fee` or a swap half, is for another party: a send refuses it, and
/// [`Self::into_bytes`] gives its bytes.
pub struct TxEnvelope {
    pub(crate) bytes: Vec<u8>,
    pub(crate) reservation: Reservation,
    pub(crate) pays_fee: bool,
}

impl TxEnvelope {
    /// The proven transaction in its tagged encoding, for another party to
    /// merge or submit.
    ///
    /// The inputs that its wallet reserved stay reserved, because the bytes
    /// can still land. The reservation ends when a sync sees the chain spend
    /// them, or when their TTL passes.
    pub fn into_bytes(self) -> Vec<u8> {
        drop(self.reservation.keep_on_drop());
        self.bytes
    }
}

impl std::fmt::Debug for FilledTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.0 {
            Fill::Transfer { .. } => "transfer",
            Fill::Assembled(_) => "assembled",
            Fill::Sealed { .. } => "sealed",
        };
        f.debug_struct("FilledTx")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for FundedTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FundedTx")
            .field("reservation", &self.reservation)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ProvenTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvenTx")
            .field("reservation", &self.reservation)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TxEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxEnvelope")
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .field("reservation", &self.reservation)
            .field("pays_fee", &self.pays_fee)
            .finish()
    }
}
