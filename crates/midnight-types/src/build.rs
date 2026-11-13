//! The ledger-typed toolkit that the SDK crates share: transaction assembly,
//! input selection, the fee loop, the reservation of a funded transaction,
//! and the conversions between the stages of a transaction.
//!
//! These items name the ledger's own types. They are outside the semver
//! promise of this crate.

use std::future::Future;
use std::sync::Arc;

use midnight_helpers::midnight_serialize::{tagged_deserialize, tagged_serialize};
use midnight_helpers::{
    BuildContext, DefaultDB, FromContext, ProofProvider, StandardTransactionInfo,
};

pub use crate::builder::{
    Cover, Funding, PreparedTransfer, TransferBuilder, balance_external, chain_time,
    execution_context, prepare_no_validate,
};
use crate::request::{RequestKind, TransactionRequest};
use crate::stage::FinalizedTx;
pub use crate::sync::TrackedUtxo;
pub use crate::transfer::{ReservedInput, SpentInputs, SpentUtxoKey};
use crate::{
    CoinPublicKey, FilledTx, FundedTx, MidnightWallet, Nullifier, ProvenTx, SignatureRequest,
    TxEnvelope, WalletError,
};

/// What a fill asks the wallet to fund.
pub enum Fill {
    /// A transfer, which the wallet selects and assembles on `context` and
    /// proves with `prover`. [`fill`] makes it only for a transfer request.
    Transfer {
        request: TransactionRequest,
        context: Arc<BuildContext>,
        prover: Arc<dyn ProofProvider<DefaultDB>>,
    },
    /// A transaction that the filler assembled, which the wallet funds in its
    /// own intents. It asks for mock fee proofs.
    Assembled(Box<StandardTransactionInfo<DefaultDB, BuildContext>>),
    /// A proven transaction, which the wallet covers in a part of its own as
    /// [`balance_external`] does. `tx_info` prices and proves the part, and
    /// carries no intents.
    Sealed {
        tx_info: Box<StandardTransactionInfo<DefaultDB, BuildContext>>,
        external: Box<FinalizedTx>,
        cover: Cover,
    },
}

/// What a wallet funded.
pub enum Funded {
    /// An unproven transaction, or the unproven part that covers a sealed
    /// transaction.
    Prepared(Box<PreparedTransfer>),
    /// A sealed transaction that the wallet had nothing to add to.
    Unchanged(Box<FinalizedTx>),
}

/// What midnight-contract adds to a transaction. Each part carries the
/// context that it executes on and the prover that proves it.
pub enum ContractPart {
    /// A deploy or a maintenance update, which carries no user circuit. The
    /// wallet pays its fee in its own intents, against mock proofs.
    Assembled(Box<StandardTransactionInfo<DefaultDB, BuildContext>>),
    /// A contract call, or a deploy with a shielded offer. The fill proves it
    /// with no fee. The wallet then covers its shortfalls in a part of its
    /// own, and spends the coins of `pins` first.
    Covered {
        tx_info: Box<StandardTransactionInfo<DefaultDB, BuildContext>>,
        pins: Vec<Nullifier>,
    },
}

impl std::fmt::Debug for ContractPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assembled(_) => f.write_str("Assembled"),
            Self::Covered { pins, .. } => f
                .debug_struct("Covered")
                .field("pins", pins)
                .finish_non_exhaustive(),
        }
    }
}

/// Fill the transaction that `request` names, for a wallet to fund.
///
/// A transfer or a sponsor executes on the context that `chain_context`
/// gives, and proves with `prover`. A contract part carries its own context
/// and prover, so its fill awaits neither. A covered contract part is proven
/// here, with no fee.
///
/// # Errors
///
/// [`WalletError::Transfer`] for a request that its kind refuses (see
/// [`TransactionRequest::without_fee`] and [`TransactionRequest::with_ttl`]),
/// and for sponsored bytes that are no transaction. A sponsor decodes its
/// bytes before it awaits `chain_context`. [`WalletError::Proving`] when the
/// proof of a covered contract part fails. The error of `chain_context`.
pub async fn fill<E: From<WalletError>>(
    request: TransactionRequest,
    chain_context: impl Future<Output = Result<Arc<BuildContext>, E>>,
    prover: Arc<dyn ProofProvider<DefaultDB>>,
) -> Result<FilledTx, E> {
    refuse_unsupported(&request)?;
    let fill = match request.kind {
        RequestKind::Sponsor(tx) => {
            let external: FinalizedTx = tagged_deserialize(&mut &tx.0[..])
                .map_err(|e| WalletError::Transfer(format!("deserialize transaction: {e}")))?;
            let tx_info =
                StandardTransactionInfo::new_from_context(chain_context.await?, prover, None);
            Fill::Sealed {
                tx_info: Box::new(tx_info),
                external: Box::new(external),
                cover: Cover::Fee,
            }
        }
        RequestKind::Contract(ContractPart::Assembled(tx_info)) => Fill::Assembled(tx_info),
        RequestKind::Contract(ContractPart::Covered { tx_info, pins }) => {
            let context = tx_info.context.clone();
            let prover = tx_info.prover.clone();
            // The fee of a user circuit is priced after its proof:
            // `mock_prove` refuses a user circuit, and a real proof on each
            // round of the fee loop would prove the circuit again.
            let external = prepare_no_validate(*tx_info, None).await?.prove().await?;
            Fill::Sealed {
                tx_info: Box::new(StandardTransactionInfo::new_from_context(
                    context, prover, None,
                )),
                external: Box::new(external),
                cover: Cover::Shortfalls {
                    pins,
                    pay_fees: request.pays_fee,
                },
            }
        }
        transfer => Fill::Transfer {
            request: TransactionRequest {
                kind: transfer,
                ..request
            },
            context: chain_context.await?,
            prover,
        },
    };
    Ok(FilledTx(fill))
}

/// Refuse a modifier that the kind of `request` cannot honour.
fn refuse_unsupported(request: &TransactionRequest) -> Result<(), WalletError> {
    let refusal = match (&request.kind, request.pays_fee, request.ttl) {
        (RequestKind::RegisterDust { .. }, false, _) => {
            "a Dust registration pays its own fee, so it cannot be built without one"
        }
        (RequestKind::Sponsor(_), false, _) => {
            "a sponsor pays the fee, so it cannot be built without one"
        }
        (RequestKind::Sponsor(_), _, Some(_)) => {
            "a sponsor keeps the TTL of the transaction that it pays for"
        }
        (RequestKind::Contract(ContractPart::Assembled(_)), false, _) => {
            "a deploy or a maintenance update pays its own fee"
        }
        (RequestKind::Contract(_), _, Some(_)) => "a contract part takes no TTL",
        _ => return Ok(()),
    };
    Err(WalletError::Transfer(refusal.into()))
}

/// Whether the transaction of `request` pays its own fee. A request made
/// [`without_fee`](TransactionRequest::without_fee) and a swap half do not.
pub fn pays_fee(request: &TransactionRequest) -> bool {
    request.pays_fee
}

/// What `tx` asks the wallet to fund.
pub fn fill_of(tx: FilledTx) -> Fill {
    tx.0
}

/// Wrap what a wallet funded, with the reservation of what it spends.
pub fn funded(funded: Funded, reservation: Reservation) -> FundedTx {
    FundedTx {
        funded,
        reservation,
    }
}

/// Prove `tx` with the prover that its fill chose, and seal it.
///
/// A proof that fails drops the reservation of `tx`, which hands its inputs
/// back to the wallet.
///
/// # Errors
///
/// [`WalletError::Proving`] when the prover fails, and
/// [`WalletError::Transfer`] when the proven part does not merge into the
/// transaction that it covers.
pub async fn prove(tx: FundedTx) -> Result<ProvenTx, WalletError> {
    let FundedTx {
        funded,
        reservation,
    } = tx;
    let tx = match funded {
        Funded::Prepared(prepared) => prepared.prove().await?,
        Funded::Unchanged(tx) => *tx,
    };
    Ok(ProvenTx { tx, reservation })
}

/// What the wallet that funded `tx` reserved for it.
pub fn spent_by(tx: &ProvenTx) -> &SpentInputs {
    tx.reservation.spent()
}

/// The unshielded inputs of the signer of `request` that hold a placeholder
/// in the intent at its segment of `tx`.
pub fn inputs_to_sign(tx: &ProvenTx, request: &SignatureRequest) -> Vec<SpentUtxoKey> {
    tx.inputs_to_sign(request)
}

/// Serialize `tx` for the node. Its reservation moves to the envelope.
///
/// # Errors
///
/// [`WalletError::Transfer`] when `tx` does not serialize. The reservation
/// then drops, which hands the inputs back.
pub fn envelope(tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
    let ProvenTx { tx, reservation } = tx;
    let mut bytes = Vec::new();
    tagged_serialize(&tx, &mut bytes)
        .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;
    Ok(TxEnvelope {
        bytes,
        reservation,
        pays_fee: true,
    })
}

/// Mark `tx` as a transaction that pays no fee, for another party to pay.
pub fn fee_less(tx: TxEnvelope) -> TxEnvelope {
    TxEnvelope {
        pays_fee: false,
        ..tx
    }
}

/// The tagged bytes of `tx` and its reservation, for a send.
///
/// # Errors
///
/// `tx` itself when it pays no fee, because the node refuses it. A drop of
/// `tx` hands its inputs back.
pub fn into_parts(tx: TxEnvelope) -> Result<(Vec<u8>, Reservation), TxEnvelope> {
    if !tx.pays_fee {
        return Err(tx);
    }
    Ok((tx.bytes, tx.reservation))
}

/// Hands the inputs of a reservation back to its wallet.
type Release = Box<dyn FnOnce(&SpentInputs) + Send + Sync>;

/// What a wallet reserved for one transaction, and how to hand it back.
///
/// The wallet makes it when it funds a transaction, and the stages of the
/// transaction carry it. A drop hands the inputs back, so an unsent
/// transaction cannot strand them. Once the node can hold the transaction,
/// [`Self::keep_on_drop`] turns it into a [`KeptReservation`].
pub struct Reservation(KeptReservation);

impl Reservation {
    /// A reservation of `spent`, which `release` hands back.
    pub fn new(
        spent: SpentInputs,
        release: impl FnOnce(&SpentInputs) + Send + Sync + 'static,
    ) -> Self {
        Self(KeptReservation {
            spent,
            release: Some(Box::new(release)),
        })
    }

    /// What this reservation holds.
    pub fn spent(&self) -> &SpentInputs {
        self.0.spent()
    }

    /// Keep the inputs reserved past a drop, because the node can hold the
    /// transaction.
    pub fn keep_on_drop(mut self) -> KeptReservation {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        std::mem::take(&mut self.0).release();
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("spent", self.spent())
            .finish_non_exhaustive()
    }
}

/// A [`Reservation`] of a transaction that the node can hold.
///
/// A drop keeps the inputs reserved, and only [`Self::release`] hands them
/// back.
#[derive(Default)]
pub struct KeptReservation {
    spent: SpentInputs,
    release: Option<Release>,
}

impl KeptReservation {
    /// What this reservation holds.
    pub fn spent(&self) -> &SpentInputs {
        &self.spent
    }

    /// Hand the inputs back now, because the transaction can no longer land.
    pub fn release(self) {
        if let Some(release) = self.release
            && !self.spent.is_empty()
        {
            release(&self.spent);
        }
    }
}

impl std::fmt::Debug for KeptReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptReservation")
            .field("spent", &self.spent)
            .finish_non_exhaustive()
    }
}

/// The wallet slot of a provider: `()` when it holds no wallet, or a
/// [`MidnightWallet`].
///
/// The local run of a circuit reads the wallet's key only when the circuit
/// calls `ownPublicKey()`, so it also runs on a provider with no wallet.
// Defined beside `MidnightWallet`: in a crate where that trait is foreign,
// rustc refuses the impl for `()` next to the blanket impl (E0119).
pub trait MaybeWallet: Send + Sync {
    /// The coin public key that `ownPublicKey()` returns, or `None` when there
    /// is no wallet.
    fn coin_public_key(&self) -> Option<CoinPublicKey>;
}

impl MaybeWallet for () {
    fn coin_public_key(&self) -> Option<CoinPublicKey> {
        None
    }
}

impl<W: MidnightWallet> MaybeWallet for W {
    fn coin_public_key(&self) -> Option<CoinPublicKey> {
        Some(self.shielded_recipient().coin_public_key)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{HashOutput, Nullifier};

    fn spending(byte: u8) -> SpentInputs {
        SpentInputs {
            shielded: vec![ReservedInput {
                input: Nullifier(HashOutput([byte; 32])),
                reserved_at: midnight_helpers::Timestamp::from_secs(1),
            }],
            ..SpentInputs::default()
        }
    }

    /// A reservation of `spent` that records what it hands back.
    fn recorded(spent: SpentInputs) -> (Reservation, Arc<Mutex<Vec<Nullifier>>>) {
        let released = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&released);
        let reservation = Reservation::new(spent, move |spent| {
            record
                .lock()
                .unwrap()
                .extend(spent.shielded.iter().map(|reserved| reserved.input));
        });
        (reservation, released)
    }

    /// A sent transaction can still land, so its handle keeps the inputs on a
    /// drop and hands them back only on a definitive rejection. A release on
    /// that drop would let a later transaction spend the same inputs.
    #[test]
    fn a_reservation_kept_on_drop_hands_back_only_on_release() {
        let (reservation, released) = recorded(spending(7));
        drop(reservation.keep_on_drop());
        assert!(released.lock().unwrap().is_empty());

        let (reservation, released) = recorded(spending(8));
        reservation.keep_on_drop().release();
        assert_eq!(*released.lock().unwrap(), [Nullifier(HashOutput([8; 32]))]);
    }

    /// A registration or a sponsor refuses `without_fee`, and a sponsor
    /// refuses a TTL. A registration or a sponsor that took `without_fee`
    /// would pay the fee that its request skips, and its envelope would be
    /// marked fee-less, which a send refuses. A sponsor that took a TTL would
    /// drop it silently: its fee intent keeps the TTL of the transaction that
    /// it pays for.
    #[test]
    fn a_registration_or_a_sponsor_refuses_to_skip_its_fee() {
        let refused = |request: TransactionRequest| {
            matches!(refuse_unsupported(&request), Err(WalletError::Transfer(_)))
        };
        assert!(refused(TransactionRequest::register_dust().without_fee()));
        assert!(refused(
            TransactionRequest::sponsor(Vec::new()).without_fee()
        ));
        assert!(refused(
            TransactionRequest::sponsor(Vec::new()).with_ttl(std::time::Duration::from_secs(60))
        ));

        assert!(refuse_unsupported(&TransactionRequest::register_dust()).is_ok());
        assert!(refuse_unsupported(&TransactionRequest::sponsor(Vec::new())).is_ok());
    }
}
