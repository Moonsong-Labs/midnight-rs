//! [`Wallet`]'s funding and signing, as a [`MidnightWallet`].

use async_trait::async_trait;
use midnight_helpers::{OsRng, Serializable, Signature};
use midnight_types::build::{
    self, Cover, Fill, Funded, PreparedTransfer, ReservedInput, SpentInputs, SpentUtxoKey,
    TransferBuilder, balance_external, prepare_no_validate,
};
use midnight_types::chain_pin::ChainView;
use midnight_types::convert::IntoLedger;
use midnight_types::{
    FilledTx, FundedTx, MidnightWallet, ProvenTx, ShieldedRecipient, SignatureRequest, TxEnvelope,
    WalletError,
};

use crate::Wallet;
use crate::pending;

/// Reserve what `prepared` spends in `wallet`'s book.
fn reserve(wallet: &mut crate::state::Wallet, prepared: &PreparedTransfer) -> SpentInputs {
    wallet.reserve_pending(
        prepared
            .dust_spends()
            .iter()
            .map(|spend| spend.old_nullifier)
            .collect(),
        prepared.spent_unshielded_inputs().to_vec(),
        prepared.spent_shielded_inputs().to_vec(),
        prepared.reserved_at(),
        prepared.expires_at(),
    );
    prepared.spent_inputs()
}

/// Refuse a pinned coin that a pending build of `wallet` holds.
///
/// The funding view leaves such a coin out, so a check against that view
/// could not tell it from a coin that the wallet does not own.
fn refuse_reserved(
    wallet: &crate::state::Wallet,
    pins: &[midnight_types::Nullifier],
) -> Result<(), WalletError> {
    let held = wallet.reserved_shielded_nullifiers();
    for pin in pins.iter().map(|pin| pin.into_ledger()) {
        if held.contains(&pin) {
            return Err(WalletError::InputsReserved {
                held: format!("shielded coin {pin:?}"),
            });
        }
    }
    Ok(())
}

/// Fund `fill` from `wallet`, and reserve what it spends.
///
/// Selection, assembly, the fee loop and the reservation all run here, under
/// the one write lock the caller holds, so two fundings cannot draw the same
/// input. On an error, nothing is reserved.
async fn fund(
    wallet: &mut crate::state::Wallet,
    fill: Fill,
) -> Result<(Funded, SpentInputs), WalletError> {
    let prepared = match fill {
        Fill::Transfer {
            request,
            context,
            prover,
        } => {
            let funding = wallet.funding(&context)?;
            TransferBuilder::new(funding, context, prover)
                .prepare(request)
                .await?
        }
        Fill::Assembled(tx_info) => {
            let funding = wallet.funding(&tx_info.context)?;
            prepare_no_validate(*tx_info, Some(&funding.dust)).await?
        }
        Fill::Sealed {
            tx_info,
            external,
            cover,
        } => {
            if let Cover::Shortfalls { pins, .. } = &cover {
                refuse_reserved(wallet, pins)?;
            }
            let funding = wallet.funding(&tx_info.context)?;
            match balance_external(*tx_info, funding, &external, &cover)? {
                Some(prepared) => prepared,
                None => return Ok((Funded::Unchanged(external), SpentInputs::default())),
            }
        }
    };
    let spent = reserve(wallet, &prepared);
    Ok((Funded::Prepared(Box::new(prepared)), spent))
}

#[async_trait]
impl MidnightWallet for Wallet {
    fn shielded_recipient(&self) -> ShieldedRecipient {
        Wallet::shielded_recipient(self)
    }

    async fn fund_transaction(
        &self,
        chain: &dyn ChainView,
        tx: FilledTx,
    ) -> Result<FundedTx, WalletError> {
        // Boxed: a debug build makes the replay future tens of kilobytes, and
        // every funding awaits it.
        Box::pin(self.sync(chain)).await?;
        let (funded, spent) = {
            let mut guard = self.state().write().await;
            fund(guard.state_mut(), build::fill_of(tx)).await?
        };
        let reservation = pending::reservation(self.pending(), spent);
        Ok(build::funded(funded, reservation))
    }

    /// Sign each placeholder of this wallet's unshielded key in `tx`,
    /// serialize `tx`, then keep its reservation across a restart.
    ///
    /// The wallet signs only the inputs that `tx` reserved, so a sponsored
    /// transaction cannot make it sign a spend or a Dust registration that it
    /// did not fund.
    async fn sign_transaction(&self, mut tx: ProvenTx) -> Result<TxEnvelope, WalletError> {
        let requests: Vec<SignatureRequest> = tx
            .signature_requests()
            .into_iter()
            .filter(|request| request.signer == self.unshielded_recipient())
            .collect();
        for request in &requests {
            refuse_unreserved(
                &build::inputs_to_sign(&tx, request),
                &build::spent_by(&tx).unshielded,
                request.segment,
            )?;
            let signature = self.signer().sign(&mut OsRng, &request.data);
            tx.add_signature(request, signature_bytes(&signature))?;
        }
        if let Some(left) = tx.signature_requests().first() {
            return Err(WalletError::Transfer(format!(
                "the transaction still needs a signature of {:?} at segment {}, \
                 which this wallet does not hold",
                left.signer, left.segment
            )));
        }
        let spent = build::spent_by(&tx).clone();
        let envelope = build::envelope(tx)?;
        if !spent.is_empty() {
            pending::lock(self.pending()).mark_proven(&spent);
        }
        Ok(envelope)
    }
}

/// Refuse to sign at `segment` unless `inputs`, the inputs that the
/// signature covers there, hold at least one input and each is in `reserved`.
///
/// An intent that spends an input of the reservation is one that this wallet
/// funded, so it carries no slot of this wallet's key that another party put
/// there.
fn refuse_unreserved(
    inputs: &[SpentUtxoKey],
    reserved: &[ReservedInput<SpentUtxoKey>],
    segment: u16,
) -> Result<(), WalletError> {
    let held = |input: &SpentUtxoKey| reserved.iter().any(|r| r.input == *input);
    if inputs.is_empty() || !inputs.iter().all(held) {
        return Err(WalletError::Transfer(format!(
            "the intent at segment {segment} needs a signature of this wallet for a spend or \
             a Dust registration that this wallet did not fund"
        )));
    }
    Ok(())
}

/// The 64-byte encoding of `signature`, which [`ProvenTx::add_signature`]
/// takes.
fn signature_bytes(signature: &Signature) -> [u8; 64] {
    let mut bytes = Vec::with_capacity(64);
    match signature {
        Signature::Schnorr(signature) => signature.serialize(&mut bytes),
        Signature::ECDSA(signature) => signature.serialize(&mut bytes),
    }
    .expect("serializing into a Vec cannot fail");
    bytes.try_into().expect("a signature encodes to 64 bytes")
}

#[cfg(test)]
mod tests {
    use midnight_helpers::Timestamp;
    use midnight_types::HashOutput;

    use super::*;

    fn utxo(byte: u8) -> SpentUtxoKey {
        SpentUtxoKey {
            intent_hash: HashOutput([byte; 32]),
            output_index: 0,
        }
    }

    /// A sponsored transaction can carry a placeholder of this wallet's key
    /// in an intent of another party: a spend of one of its UTXOs, or a
    /// registration of its NIGHT address to another Dust key. A wallet that
    /// signed every request of its key would sign them, and lose the UTXO or
    /// the Dust it generates.
    #[test]
    fn the_wallet_signs_only_for_inputs_that_it_reserved() {
        let reserved = [ReservedInput {
            input: utxo(1),
            reserved_at: Timestamp::from_secs(100),
        }];

        assert!(refuse_unreserved(&[utxo(1)], &reserved, 1).is_ok());
        for foreign in [&[utxo(1), utxo(2)][..], &[]] {
            assert!(
                matches!(
                    refuse_unreserved(foreign, &reserved, 1),
                    Err(WalletError::Transfer(_))
                ),
                "{foreign:?}"
            );
        }
    }
}
