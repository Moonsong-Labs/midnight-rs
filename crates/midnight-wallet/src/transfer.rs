use futures_util::FutureExt;

use std::str::FromStr;
use std::sync::Arc;

use midnight_helpers::{
    BuildUtxoOutput, BuildUtxoSpend, CoinSelectionStrategy, DefaultDB, DustActions, DustLocalState,
    DustRegistrationBuilder, DustSpend, DustWallet, FromContext, HashMapStorage, InputInfo, Intent,
    IntentInfo, LedgerContext, LedgerParameters, NIGHT, Nullifier, OfferInfo, OutputInfo,
    PedersenRandomness, ProofPreimageMarker, ProofProvider, Segment, ShieldedTokenType,
    ShieldedWallet, Signature, Sp, SplittableRng, StandardTrasactionInfo, StdRng, Timestamp,
    TokenType, Transaction, UnshieldedOfferInfo, UnshieldedTokenType, UnshieldedWallet,
    UtxoOutputInfo, UtxoSpendInfo, WalletAddress, WalletSeed,
};

use crate::WalletError;
use crate::network::Network;
use crate::state::{TrackedUtxo, Wallet};

type UnprovenTx = Transaction<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>;
type FinalizedTx = midnight_helpers::FinalizedTransaction<DefaultDB>;

pub struct TransferResult {
    pub tx_bytes: Vec<u8>,
    /// Unshielded UTXO inputs consumed by this transaction. Pass to
    /// [`crate::Wallet::reserve_pending`] together with `dust_batches` so
    /// subsequent in-process builds don't re-select the same inputs before
    /// the indexer surfaces the spend events.
    pub spent_unshielded_inputs: Vec<SpentUtxoKey>,
    /// Shielded (Zswap) coin nullifiers consumed by this transaction. Pass to
    /// [`crate::Wallet::reserve_pending`] so subsequent in-process builds don't
    /// re-select the same coins before the indexer confirms the spend.
    pub spent_shielded_inputs: Vec<Nullifier>,
    /// Dust batches that funded this transaction's fees. Each batch's
    /// `(spends, updated_state)` pair came from one `speculative_spend`
    /// call and must be kept together for the new `mark_spent` API.
    /// Same caveat as `spent_unshielded_inputs` — pass to
    /// [`crate::Wallet::reserve_pending`] for double-build prevention.
    pub dust_batches: Vec<DustSpendBatch>,
    /// Deterministic Dust fee the chain will charge for this transaction, in
    /// SPECK (`1 DUST = 10^15 SPECK`). Computed via
    /// `Transaction::fees(&ledger.parameters, false)` against the parameters
    /// the build pipeline saw — matches what the node's own estimation RPC
    /// returns and what the indexer later reports as `paidFees` for an
    /// accepted, included transaction.
    pub fee_speck: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentUtxoKey {
    pub intent_hash: String,
    pub output_index: u32,
}

/// What one build spends, in the form a reservation takes.
///
/// A build reserves its inputs so a later build in the same process does not
/// re-select them before the indexer surfaces the spend. Reserve them, and
/// hand them back if the build will never reach the chain.
#[derive(Default, Clone)]
pub struct SpentInputs {
    /// The Dust batches that fund the fee.
    pub dust_batches: Vec<DustSpendBatch>,
    /// The unshielded UTXOs the build spends.
    pub unshielded: Vec<SpentUtxoKey>,
    /// The shielded coins the build spends, by nullifier.
    pub shielded: Vec<Nullifier>,
}

impl SpentInputs {
    /// The Dust a build drew, for one that spends nothing else.
    pub fn from_dust(dust_batches: Vec<DustSpendBatch>) -> Self {
        Self {
            dust_batches,
            ..Self::default()
        }
    }

    /// The shielded coins a build pinned, for one that spends nothing else.
    pub fn from_shielded(shielded: Vec<Nullifier>) -> Self {
        Self {
            shielded,
            ..Self::default()
        }
    }

    /// The nullifier of every Dust spend here, which is how
    /// [`crate::Wallet::release_pending`] names them.
    pub fn dust_nullifiers(&self) -> Vec<midnight_helpers::DustNullifier> {
        self.dust_batches
            .iter()
            .flat_map(|b| b.spends.iter().map(|s| s.old_nullifier))
            .collect()
    }
}

impl From<&TransferResult> for SpentInputs {
    fn from(result: &TransferResult) -> Self {
        Self {
            dust_batches: result.dust_batches.clone(),
            unshielded: result.spent_unshielded_inputs.clone(),
            shielded: result.spent_shielded_inputs.clone(),
        }
    }
}

/// What a transfer build reads from the wallet that funds it.
///
/// A build signs with the seed, validates a recipient address against the
/// network, and a dust registration additionally needs the dust parameters,
/// the dust public key to register, and the tNIGHT UTXOs to choose one from.
/// Everything else a build needs is in the [`LedgerContext`] it is handed, so
/// these five readings are the whole of the wallet a build depends on.
pub trait BuildInputs: Send + Sync {
    /// The seed the build signs and derives with.
    fn seed(&self) -> &WalletSeed;

    /// The network identifier a recipient address must match.
    fn network(&self) -> &str;

    /// The ledger parameters a dust registration computes its fee allowance
    /// from.
    fn parameters(&self) -> &LedgerParameters;

    /// The dust wallet whose public key a registration names.
    fn dust_wallet(&self) -> &DustWallet<DefaultDB>;

    /// The unshielded UTXOs a dust registration picks its tNIGHT from.
    fn unshielded_utxos(&self) -> &[TrackedUtxo];
}

impl BuildInputs for Wallet {
    fn seed(&self) -> &WalletSeed {
        Wallet::seed(self)
    }

    fn network(&self) -> &str {
        Wallet::network(self)
    }

    fn parameters(&self) -> &LedgerParameters {
        Wallet::parameters(self)
    }

    fn dust_wallet(&self) -> &DustWallet<DefaultDB> {
        Wallet::dust_wallet(self)
    }

    fn unshielded_utxos(&self) -> &[TrackedUtxo] {
        Wallet::unshielded_utxos(self)
    }
}

/// One transfer a build can make.
///
/// Data rather than a closure over the builder, so a caller can hand a build
/// to a wallet it reaches only through a trait.
pub enum TransferKind {
    /// A shielded (Zswap) transfer. See
    /// [`TransferBuilder::prepare_shielded`].
    Shielded {
        token_type: ShieldedTokenType,
        amount: u128,
        /// A bech32 shielded address.
        recipient: String,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// An unshielded (UTXO) transfer. See
    /// [`TransferBuilder::prepare_unshielded`].
    Unshielded {
        token_type: UnshieldedTokenType,
        amount: u128,
        /// A bech32 unshielded address.
        recipient: String,
        /// False leaves the transaction fee-less, for another wallet to fund.
        pay_fees: bool,
    },
    /// One half of a two-party shielded swap, always fee-less. See
    /// [`TransferBuilder::prepare_shielded_swap`].
    ShieldedSwap {
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    },
    /// A dust-address registration. See
    /// [`TransferBuilder::prepare_register_dust`].
    DustRegistration {
        /// Fallback creation time for the UTXOs whose own creation time the
        /// indexer did not report.
        utxo_ctime: Option<u64>,
    },
}

/// One build, as data: what to make, and how to choose the inputs that make
/// it.
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
    /// [`TransferBuilder::with_coin_selection`].
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

pub struct TransferBuilder<'a> {
    state: &'a dyn BuildInputs,
    context: Arc<LedgerContext<DefaultDB>>,
    proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    coin_selection: CoinSelectionStrategy,
}

impl<'a> TransferBuilder<'a> {
    pub fn new(
        state: &'a dyn BuildInputs,
        context: Arc<LedgerContext<DefaultDB>>,
        proof_provider: Arc<dyn ProofProvider<DefaultDB>>,
    ) -> Self {
        Self {
            state,
            context,
            proof_provider,
            coin_selection: CoinSelectionStrategy::default(),
        }
    }

    /// Order the coins and UTXOs this build draws on.
    ///
    /// [`CoinSelectionStrategy::LargestFirst`], the default, spends the fewest
    /// inputs, which matters because every shielded input carries its own proof
    /// and is charged for separately. [`CoinSelectionStrategy::SmallestFirst`]
    /// spends the most, absorbing small coins that the default would leave
    /// untouched indefinitely, at the cost of a larger and more expensive
    /// transaction. Nothing caps how many inputs the latter draws, so prefer it
    /// for a deliberate cleanup rather than for ordinary payments.
    ///
    /// Neither ordering is a good default, since one never absorbs small coins
    /// and the other is unbounded. TODO: replace both with best-fit selection,
    /// https://github.com/Moonsong-Labs/midnight-rs/issues/145
    pub fn with_coin_selection(mut self, strategy: CoinSelectionStrategy) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// Prepare the transfer a request names, with the input ordering it asks
    /// for. Dispatches to the matching `prepare_*` method below, each of which
    /// documents what it builds.
    pub async fn prepare(self, request: TransferRequest) -> Result<PreparedTransfer, WalletError> {
        let TransferRequest {
            kind,
            coin_selection,
        } = request;
        let builder = self.with_coin_selection(coin_selection);
        match kind {
            TransferKind::Shielded {
                token_type,
                amount,
                recipient,
                pay_fees,
            } => {
                builder
                    .prepare_shielded(token_type, amount, &recipient, pay_fees)
                    .await
            }
            TransferKind::Unshielded {
                token_type,
                amount,
                recipient,
                pay_fees,
            } => {
                builder
                    .prepare_unshielded(token_type, amount, &recipient, pay_fees)
                    .await
            }
            TransferKind::ShieldedSwap {
                give_token,
                give_amount,
                receive_token,
                receive_amount,
            } => {
                builder
                    .prepare_shielded_swap(give_token, give_amount, receive_token, receive_amount)
                    .await
            }
            TransferKind::DustRegistration { utxo_ctime } => {
                builder.prepare_register_dust(utxo_ctime).await
            }
        }
    }

    /// Build a shielded (ZSwap) transfer transaction.
    ///
    /// `recipient` is a bech32 shielded address (e.g.
    /// `mn_shield-addr_undeployed1...`). Only the public material is needed:
    /// the address carries the recipient's `coin_public_key` and
    /// `enc_public_key`, which is all the chain needs to construct the output
    /// coin commitment and encrypt the coin info for them.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for a multi-party flow where another
    /// wallet pays the fees (the `.without_dust()` path). It is not submittable
    /// on its own; hand it to the fee payer, who completes it with
    /// `MidnightProvider::balance_transaction` (in `midnight-provider`) and
    /// submits.
    ///
    /// `amount` may span several coins: inputs are selected up front with
    /// [`InputInfo::coins_to_cover_value`] and the remainder returns to this
    /// wallet as a change output, so a payment is bounded by the wallet's total
    /// balance in `token_type` rather than by its largest single coin. Sending
    /// the full balance to this wallet's own address therefore consolidates it
    /// into one coin, and sending part of a coin splits it. The spent coins'
    /// nullifiers are surfaced in [`PreparedTransfer::spent_shielded_inputs`] so
    /// the caller can reserve them.
    /// Stops before proving. Reserve the inputs the returned
    /// [`PreparedTransfer`] reports, then call [`PreparedTransfer::prove`]:
    /// the transaction bytes exist only after that. Reserving first is what
    /// stops another build in this process selecting the same inputs.
    pub async fn prepare_shielded(
        self,
        token_type: ShieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let from_seed = self.state.seed().clone();
        let recipient_wallet = parse_shielded_recipient(recipient, self.state.network())?;

        // Select the coins here rather than leaving `nullifier: None` for the
        // build to resolve: that path binds the single smallest coin covering
        // `amount` and then substitutes the coin's own value for the requested
        // one, which caps a payment at the largest single coin and drops
        // whatever that coin holds beyond `amount` with no output to claim it.
        let (inputs, change) = InputInfo::coins_to_cover_value(
            self.context.clone(),
            from_seed.clone(),
            amount,
            token_type,
            self.coin_selection,
        )
        .map_err(|e| WalletError::Transfer(format!("shielded coin selection: {e}")))?;

        // Every input from `coins_to_cover_value` carries a pinned nullifier;
        // error rather than drop one silently, since a missing nullifier would
        // leave the coin unreserved and defeat the double-spend protection.
        let spent_shielded_inputs: Vec<Nullifier> = inputs
            .iter()
            .map(|i| {
                i.nullifier.ok_or_else(|| {
                    WalletError::Transfer(
                        "selected shielded coin has no nullifier to reserve".into(),
                    )
                })
            })
            .collect::<Result<_, _>>()?;

        let mut outputs: Vec<Box<dyn midnight_helpers::BuildOutput<DefaultDB>>> =
            vec![Box::new(OutputInfo {
                destination: recipient_wallet,
                token_type,
                value: amount,
            })];

        // Change back to this wallet, only if any.
        if change > 0 {
            outputs.push(Box::new(OutputInfo {
                destination: from_seed.clone(),
                token_type,
                value: change,
            }));
        }

        // The wallet performs the spends, so the offer holds finished inputs
        // rather than a seed it would resolve during `build`.
        let context = self.context.clone();
        let mut tx_info =
            StandardTrasactionInfo::new_from_context(self.context, self.proof_provider, None);
        let prepared = crate::prepared_input::prepare_shielded_inputs(
            &context,
            &from_seed,
            &spent_shielded_inputs,
            &mut tx_info.rng.split(),
        )?;

        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: prepared
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn midnight_helpers::BuildInput<DefaultDB>>)
                .collect(),
            outputs,
            transients: vec![],
        });
        // Fund the Dust fee from our own seed unless this is a Dustless build
        // (another wallet will sponsor the fees).
        if pay_fees {
            tx_info.set_funding_seeds(vec![from_seed]);
        }
        tx_info.use_mock_proofs_for_fees(true);

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.spent_shielded_inputs = spent_shielded_inputs;
        Ok(prepared)
    }

    /// Build one half of a native two-party shielded token swap: a proven,
    /// fee-less Zswap offer that spends `give_amount` of `give_token` and
    /// creates an output for `receive_amount` of `receive_token` payable to
    /// this wallet.
    ///
    /// The half is intentionally unbalanced: its offer deltas are
    /// `give_token = +give_amount` (surplus given up) and
    /// `receive_token = -receive_amount` (deficit to be filled). It cannot
    /// stand alone. The counterparty builds the exact mirror
    /// (`shielded_swap(receive_token, receive_amount, give_token, give_amount)`),
    /// and merging the two cancels both tokens into a balanced transaction that
    /// a sponsor funds (`balance_transaction`) and submits.
    ///
    /// Because the half is unbalanced it can't self-fund its Dust, so it is
    /// always fee-less: no funding seed is set and the returned transaction is
    /// inherently Dustless. Give-side coins are selected up front with
    /// [`InputInfo::coins_to_cover_value`]; the resulting change (if any) goes
    /// back to this wallet, mirroring [`Self::prepare_unshielded`]'s change handling in
    /// the shielded domain. The spent coins' nullifiers are surfaced in
    /// [`PreparedTransfer::spent_shielded_inputs`] so the caller can reserve them.
    /// Stops before proving. Reserve the inputs the returned
    /// [`PreparedTransfer`] reports, then call [`PreparedTransfer::prove`]:
    /// the transaction bytes exist only after that. Reserving first is what
    /// stops another build in this process selecting the same inputs.
    pub async fn prepare_shielded_swap(
        self,
        give_token: ShieldedTokenType,
        give_amount: u128,
        receive_token: ShieldedTokenType,
        receive_amount: u128,
    ) -> Result<PreparedTransfer, WalletError> {
        let seed = self.state.seed().clone();

        // Select give-side coins covering `give_amount`; `change` is the
        // remainder handed back to this wallet below.
        let (give_inputs, change) = InputInfo::coins_to_cover_value(
            self.context.clone(),
            seed.clone(),
            give_amount,
            give_token,
            self.coin_selection,
        )
        .map_err(|e| WalletError::Transfer(format!("shielded coin selection: {e}")))?;

        // Every input from `coins_to_cover_value` carries a pinned nullifier;
        // error rather than drop one silently, since a missing nullifier would
        // leave the coin unreserved and defeat the double-spend protection.
        let spent_shielded_inputs: Vec<Nullifier> = give_inputs
            .iter()
            .map(|i| {
                i.nullifier.ok_or_else(|| {
                    WalletError::Transfer(
                        "selected shielded coin has no nullifier to reserve".into(),
                    )
                })
            })
            .collect::<Result<_, _>>()?;

        // Output 1: the received token, to this wallet. Destination is our own
        // seed so the build's `watch_for` tracks the incoming coin.
        let mut outputs: Vec<Box<dyn midnight_helpers::BuildOutput<DefaultDB>>> =
            vec![Box::new(OutputInfo {
                destination: seed.clone(),
                token_type: receive_token,
                value: receive_amount,
            })];

        // Output 2: the give-side change back to this wallet, only if any.
        if change > 0 {
            outputs.push(Box::new(OutputInfo {
                destination: seed.clone(),
                token_type: give_token,
                value: change,
            }));
        }

        // The wallet performs the spends, so the offer holds finished inputs
        // rather than a seed it would resolve during `build`.
        let context = self.context.clone();
        let mut tx_info =
            StandardTrasactionInfo::new_from_context(self.context, self.proof_provider, None);
        let prepared = crate::prepared_input::prepare_shielded_inputs(
            &context,
            &seed,
            &spent_shielded_inputs,
            &mut tx_info.rng.split(),
        )?;

        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: prepared
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn midnight_helpers::BuildInput<DefaultDB>>)
                .collect(),
            outputs,
            transients: vec![],
        });
        // No funding seed: an unbalanced half can't self-fund its Dust, so this
        // is inherently fee-less. `build_no_validate` proves the offer directly
        // (no token or Dust balancing) precisely because no funding seed is set.
        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.spent_shielded_inputs = spent_shielded_inputs;
        Ok(prepared)
    }

    /// Build an unshielded (UTXO) transfer transaction.
    ///
    /// `recipient` is a bech32 unshielded address (e.g.
    /// `mn_addr_undeployed1...`). Only the recipient's `user_address` (the
    /// public part) is needed; the chain derives the output's owner field
    /// directly from it. The change output, if any, goes back to the
    /// sender's own seed-derived address.
    ///
    /// When `pay_fees` is false the build skips Dust entirely, yielding a
    /// fee-unbalanced transaction for another wallet to sponsor (the
    /// `.without_dust()` path); see [`Self::prepare_shielded`] for the multi-party flow.
    /// Stops before proving. Reserve the inputs the returned
    /// [`PreparedTransfer`] reports, then call [`PreparedTransfer::prove`]:
    /// the transaction bytes exist only after that. Reserving first is what
    /// stops another build in this process selecting the same inputs.
    pub async fn prepare_unshielded(
        self,
        token_type: UnshieldedTokenType,
        amount: u128,
        recipient: &str,
        pay_fees: bool,
    ) -> Result<PreparedTransfer, WalletError> {
        let from_seed = self.state.seed().clone();
        let recipient_wallet = parse_unshielded_recipient(recipient, self.state.network())?;

        let (spend_infos, change) = UtxoSpendInfo::utxos_to_cover_value(
            self.context.clone(),
            from_seed.clone(),
            amount,
            token_type,
            self.coin_selection,
        )
        .map_err(|e| WalletError::Transfer(format!("utxo selection: {e}")))?;

        let spent_unshielded_inputs: Vec<SpentUtxoKey> = spend_infos
            .iter()
            .filter_map(|s| {
                let intent_hash = s.intent_hash.as_ref()?;
                let output_index = s.output_number?;
                Some(SpentUtxoKey {
                    intent_hash: hex::encode(intent_hash.0.0),
                    output_index,
                })
            })
            .collect();

        let mut outputs: Vec<Box<dyn midnight_helpers::BuildUtxoOutput<DefaultDB>>> =
            vec![Box::new(UtxoOutputInfo {
                value: amount,
                owner: recipient_wallet,
                token_type,
            })];

        if change > 0 {
            outputs.push(Box::new(UtxoOutputInfo {
                value: change,
                owner: from_seed.clone(),
                token_type,
            }));
        }

        let unshielded_offer = UnshieldedOfferInfo {
            inputs: spend_infos
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn midnight_helpers::BuildUtxoSpend<DefaultDB>>)
                .collect(),
            outputs,
        };

        let intent_info: IntentInfo<DefaultDB> = IntentInfo {
            guaranteed_unshielded_offer: Some(unshielded_offer),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        let mut tx_info =
            StandardTrasactionInfo::new_from_context(self.context, self.proof_provider, None);
        tx_info.add_intent(1, Box::new(intent_info));
        tx_info.set_guaranteed_offer(OfferInfo {
            inputs: vec![],
            outputs: vec![],
            transients: vec![],
        });
        // Fund the Dust fee from our own seed unless this is a Dustless build
        // (another wallet will sponsor the fees).
        if pay_fees {
            tx_info.set_funding_seeds(vec![from_seed]);
        }
        tx_info.use_mock_proofs_for_fees(true);

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.spent_unshielded_inputs = spent_unshielded_inputs;
        Ok(prepared)
    }

    /// Build a dust address registration transaction.
    ///
    /// Spends and re-creates one tNIGHT UTXO while registering the dust
    /// address. Uses "generationless fee availability" (virtual dust accrued
    /// by holding tNIGHT) to self-fund the registration fee, so a wallet needs
    /// no dust to register.
    ///
    /// One UTXO per call, because a second unshielded input in the same
    /// transaction costs more verification than its bytes buy in dismissal
    /// allowance, and the ledger refuses it with
    /// `FeeCalculation(OutsideTimeToDismiss)`, custom error 168.
    ///
    /// A registration covers the UTXO it spends, and it registers the address,
    /// so tNIGHT arriving afterwards generates dust with no further call. It
    /// does not reach back: tNIGHT the wallet already held stays outside dust
    /// generation until a registration spends it. Call this until it reports
    /// that every tNIGHT UTXO generates dust, which is
    /// [`crate::balance::DustBalance::unregistered_night_utxos`] calls.
    ///
    /// The UTXO chosen is the one carrying the most generationless dust, which
    /// is value against age, so the fee has the most room.
    ///
    /// Skips tNIGHT UTXOs that already generate dust, and errors when every
    /// one of them does. The ledger grants no generationless availability for
    /// such a UTXO, so a registration that included one would declare a fee
    /// allowance the node rejects.
    ///
    /// `utxo_ctime` is a fallback creation timestamp (seconds since epoch),
    /// used only for UTXOs whose own creation time the indexer did not report.
    /// If `None`, those UTXOs fall back to `now - 1 hour`.
    /// Stops before proving. Reserve the inputs the returned
    /// [`PreparedTransfer`] reports, then call [`PreparedTransfer::prove`]:
    /// the transaction bytes exist only after that. Reserving first is what
    /// stops another build in this process selecting the same inputs.
    pub async fn prepare_register_dust(
        self,
        utxo_ctime: Option<u64>,
    ) -> Result<PreparedTransfer, WalletError> {
        let seed = self.state.seed().clone();

        let all_night: Vec<_> = self
            .state
            .unshielded_utxos()
            .iter()
            .filter(|u| u.is_night())
            .collect();

        if all_night.is_empty() {
            return Err(WalletError::Transfer(
                "no tNIGHT UTXOs available for dust registration".into(),
            ));
        }

        let dust_params = &self.state.parameters().dust;
        let now = self.context.latest_block_context().tblock;
        let fallback_ctime = match utxo_ctime {
            Some(t) => Timestamp::from_secs(t),
            None => Timestamp::from_secs(now.to_secs().saturating_sub(3600)),
        };
        let ctime_of = |u: &TrackedUtxo| {
            u.ctime
                .and_then(|t| u64::try_from(t).ok())
                .map_or(fallback_ctime, Timestamp::from_secs)
        };

        // One input, the one carrying the most generationless dust. A second
        // unshielded input costs more verification than its bytes buy in
        // dismissal allowance, and the ledger refuses the transaction with
        // `FeeCalculation(OutsideTimeToDismiss)`, which reaches a client as
        // custom error 168.
        //
        // A UTXO whose creation time the indexer left out is ranked below
        // every UTXO that has one. Its age is a guess, and a guess that runs
        // ahead of the truth declares an allowance the ledger will not grant.
        let chosen = choose_registration_input(&all_night, |u| {
            generationless_fee_availability(
                &[(u.value, ctime_of(u))],
                dust_params.night_dust_ratio,
                dust_params.generation_decay_rate,
                now,
            )
        })
        .ok_or_else(|| {
            WalletError::Transfer(
                "every tNIGHT UTXO already generates dust: this address is registered".into(),
            )
        })?;
        let night_utxos = [chosen];

        let inputs: Vec<Box<dyn BuildUtxoSpend<DefaultDB>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoSpendInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                    intent_hash: utxo
                        .intent_hash
                        .as_deref()
                        .and_then(crate::state::parse_intent_hash_hex),
                    output_number: utxo.output_index.map(|i| i as u32),
                };
                Box::new(info) as Box<dyn BuildUtxoSpend<DefaultDB>>
            })
            .collect();

        let outputs: Vec<Box<dyn BuildUtxoOutput<DefaultDB>>> = night_utxos
            .iter()
            .map(|utxo| {
                let info = UtxoOutputInfo {
                    value: utxo.value,
                    owner: seed.clone(),
                    token_type: NIGHT,
                };
                Box::new(info) as Box<dyn BuildUtxoOutput<DefaultDB>>
            })
            .collect();

        // The registered input belongs in the guaranteed offer. The ledger sums
        // the availability backing `allow_fee_payment` over the parent
        // intent's guaranteed inputs only, and creates the initial dust
        // outputs from that same offer, so a UTXO in the fallible leg is
        // declared but never counted and the node rejects the registration.
        let intent = IntentInfo {
            guaranteed_unshielded_offer: Some(UnshieldedOfferInfo { inputs, outputs }),
            fallible_unshielded_offer: None,
            actions: vec![],
        };

        // Declared over the same input the guaranteed offer carries. The
        // ledger sums availability over that offer's inputs, so a declaration
        // covering anything else is rejected.
        let allow_fee_payment = generationless_fee_availability(
            &[(chosen.value, ctime_of(chosen))],
            dust_params.night_dust_ratio,
            dust_params.generation_decay_rate,
            now,
        );

        let unshielded = UnshieldedWallet::default(seed.clone());
        let signing_key = unshielded.signing_key().clone();
        let dust_public_key = self.state.dust_wallet().public_key;

        let mut tx_info = StandardTrasactionInfo::new_from_context(
            self.context.clone(),
            self.proof_provider.clone(),
            None,
        );
        tx_info.add_intent(Segment::Fallible.into(), Box::new(intent));
        tx_info.add_dust_registration(DustRegistrationBuilder {
            signing_key,
            dust_address: Some(dust_public_key),
            allow_fee_payment,
        });
        tx_info.use_mock_proofs_for_fees(true);

        // Capture the spent key so callers can avoid re-selecting it via
        // `Wallet::remove_unshielded_spent` before the indexer confirms.
        let spent_unshielded_inputs: Vec<SpentUtxoKey> = night_utxos
            .iter()
            .filter_map(|u| {
                Some(SpentUtxoKey {
                    intent_hash: u.intent_hash.clone()?,
                    output_index: u.output_index? as u32,
                })
            })
            .collect();

        let mut prepared = prepare_no_validate(tx_info).await?;
        prepared.spent_unshielded_inputs = spent_unshielded_inputs;
        Ok(prepared)
    }
}

/// Pick the tNIGHT UTXO a registration should spend, or `None` when every one
/// of them already generates dust.
///
/// Ranked by the generationless dust the UTXO carries, which is its value
/// against its age, so the registration fee has the most room. A UTXO whose
/// creation time the indexer left out ranks below every UTXO that has one:
/// `availability` has to guess its age, and a guess that runs ahead of the
/// truth declares an allowance the ledger will not grant.
fn choose_registration_input<'a>(
    night_utxos: &[&'a TrackedUtxo],
    availability: impl Fn(&TrackedUtxo) -> u128,
) -> Option<&'a TrackedUtxo> {
    night_utxos
        .iter()
        .copied()
        .filter(|u| !u.is_registered_for_dust())
        .max_by_key(|u| (u.ctime.is_some(), availability(u)))
}

/// Mirror of the ledger's `generationless_fee_availability`, which ages every
/// UTXO from its own creation time against the `DustActions.ctime` the builder
/// stamps with `now`. A shared age would over-declare for the younger UTXOs and
/// the node would reject the registration.
fn generationless_fee_availability(
    utxos: &[(u128, Timestamp)],
    night_dust_ratio: u64,
    generation_decay_rate: u32,
    now: Timestamp,
) -> u128 {
    utxos
        .iter()
        .map(|&(value, ctime)| {
            let dt = u128::try_from((now - ctime).as_seconds()).unwrap_or(0);
            let vfull = value.saturating_mul(night_dust_ratio as u128);
            let rate = value.saturating_mul(generation_decay_rate as u128);
            u128::min(dt.saturating_mul(rate), vfull)
        })
        .fold(0u128, |a, b| a.saturating_add(b))
}

/// A transfer whose inputs are selected and whose Dust fee is balanced, but
/// which is not yet proven.
///
/// The wallet is needed only to produce this. Proving reads the build context
/// alone, so a caller can reserve what this reports, release the wallet, and
/// prove afterwards.
pub struct PreparedTransfer {
    tx_info: StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    dust_batches: Vec<DustSpendBatch>,
    spent_unshielded_inputs: Vec<SpentUtxoKey>,
    spent_shielded_inputs: Vec<Nullifier>,
}

impl PreparedTransfer {
    /// The Dust batches that will fund this build's fee.
    pub fn dust_batches(&self) -> &[DustSpendBatch] {
        &self.dust_batches
    }

    /// The unshielded UTXOs this build will spend.
    pub fn spent_unshielded_inputs(&self) -> &[SpentUtxoKey] {
        &self.spent_unshielded_inputs
    }

    /// The shielded coins this build will spend, by nullifier.
    pub fn spent_shielded_inputs(&self) -> &[Nullifier] {
        &self.spent_shielded_inputs
    }

    /// Everything this build will spend, in the form a reservation takes.
    pub fn spent_inputs(&self) -> SpentInputs {
        SpentInputs {
            dust_batches: self.dust_batches.clone(),
            unshielded: self.spent_unshielded_inputs.clone(),
            shielded: self.spent_shielded_inputs.clone(),
        }
    }

    /// Prove the transaction and serialize it. The slowest step in a build,
    /// and it touches no wallet.
    pub async fn prove(self) -> Result<TransferResult, WalletError> {
        let PreparedTransfer {
            mut tx_info,
            tx,
            dust_batches,
            spent_unshielded_inputs,
            spent_shielded_inputs,
        } = self;

        // Keep a handle to the ledger context so we can read `parameters`
        // after proving to compute the fee. `Arc::clone` is cheap and the lock
        // inside `with_ledger_state` is held only for the closure.
        let context = tx_info.context.clone();
        let finalized = prove_tx_no_validate(&mut tx_info, tx).await?;

        let mut bytes = Vec::new();
        midnight_helpers::midnight_serialize::tagged_serialize(&finalized, &mut bytes)
            .map_err(|e| WalletError::Transfer(format!("serialize: {e}")))?;

        // Mirrors the node's own estimation RPC: `enforce_time_to_dismiss =
        // false`, i.e. report the deterministic SPECK cost without the
        // chain-side mempool-eviction check. The chain charges the same number
        // at inclusion; if the tx exceeds the eviction-time bound, that
        // surfaces at submit, not here.
        let fee_speck = context
            .with_ledger_state(|s| finalized.fees(&s.parameters, false))
            .map_err(transfer_err("fees"))?;

        Ok(TransferResult {
            tx_bytes: bytes,
            spent_unshielded_inputs,
            spent_shielded_inputs,
            dust_batches,
            fee_speck,
        })
    }
}

fn transfer_err<E: std::fmt::Debug>(ctx: &str) -> impl FnOnce(E) -> WalletError + '_ {
    move |e| WalletError::Transfer(format!("{ctx}: {e:?}"))
}

/// The proven transaction plus the dust batches that funded it.
///
/// Each [`DustSpendBatch`] groups per-seed `(spends, updated_state)` from a
/// single `speculative_spend` call, since the new helpers `mark_spent` API
/// requires that pair together. Callers pass these batches to
/// [`crate::Wallet::reserve_pending`] so subsequent in-process builds
/// (before the indexer surfaces the spend events) don't re-select the same
/// dust UTXOs.
pub struct BuiltTransaction {
    pub finalized: FinalizedTx,
    pub dust_batches: Vec<DustSpendBatch>,
}

/// Assemble the unproven transaction `tx_info` describes, with its offers and
/// intents built but no Dust attached and nothing proven. Also returns the
/// chain time it read and the TTL derived from it, which fee balancing needs.
async fn assemble_unproven(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
) -> Result<(UnprovenTx, Timestamp, Timestamp), WalletError> {
    let now = tx_info.context.latest_block_context().tblock;
    let delay = tx_info
        .context
        .with_ledger_state(|ls| ls.parameters.global_ttl);
    let ttl = now + delay;

    let guaranteed_offer = tx_info
        .guaranteed_offer
        .as_mut()
        .map(|gc| gc.build(&mut tx_info.rng, tx_info.context.clone()))
        .transpose()
        .map_err(transfer_err("build guaranteed offer"))?;

    let mut fallible_offers_vec = Vec::new();
    for (segment_id, offer_info) in tx_info.fallible_offers.iter_mut() {
        let offer = offer_info
            .build(&mut tx_info.rng, tx_info.context.clone())
            .map_err(transfer_err("build fallible offer"))?;
        fallible_offers_vec.push((*segment_id, offer));
    }
    let fallible_offer = fallible_offers_vec.into_iter().collect();

    let mut intents = HashMapStorage::<
        u16,
        Intent<Signature, ProofPreimageMarker, PedersenRandomness, DefaultDB>,
        DefaultDB,
    >::new();
    for (segment_id, intent_info) in tx_info.intents.iter_mut() {
        let intent = intent_info
            .build(&mut tx_info.rng, ttl, tx_info.context.clone(), *segment_id)
            .await;
        intents = intents.insert(*segment_id, intent);
    }

    let network_id = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?
        .network_id
        .clone();

    let tx = Transaction::new(network_id, intents, guaranteed_offer, fallible_offer);

    Ok((tx, now, ttl))
}

/// Build and prove a transaction without the helpers' final `well_formed()`
/// check. The chain validates with its own `root_history`; matching that
/// locally would require a 55MB+ global `DustState`. Matches midnight-js.
pub async fn build_no_validate(
    mut tx_info: StandardTrasactionInfo<DefaultDB>,
) -> Result<BuiltTransaction, WalletError> {
    let (tx, now, ttl) = assemble_unproven(&mut tx_info).await?;

    if tx_info.funding_seeds.is_empty() && tx_info.dust_registrations.is_empty() {
        let finalized = prove_tx_no_validate(&mut tx_info, tx).await?;
        Ok(BuiltTransaction {
            finalized,
            dust_batches: Vec::new(),
        })
    } else {
        pay_fees_no_validate(&mut tx_info, tx, now, ttl).await
    }
}

/// Select the inputs and balance the fee, and stop before proving.
///
/// Everything here reads the build context rather than the wallet, so a caller
/// holding a wallet can prepare, record what the build will spend, release the
/// wallet, and prove afterwards. Proving needs no wallet and is far the slowest
/// step, so keeping it out of that span is what stops one build blocking every
/// other consumer.
///
/// Requires mock fee proofs when the build funds itself. Balancing without
/// them has to prove each round, which is the cost preparing exists to avoid.
pub async fn prepare_no_validate(
    mut tx_info: StandardTrasactionInfo<DefaultDB>,
) -> Result<PreparedTransfer, WalletError> {
    let (tx, now, ttl) = assemble_unproven(&mut tx_info).await?;

    let (tx, dust_batches) =
        if tx_info.funding_seeds.is_empty() && tx_info.dust_registrations.is_empty() {
            (tx, Vec::new())
        } else if tx_info.mock_proofs_for_fees {
            balance_fees_with_mocks(&mut tx_info, tx, now, ttl)?
        } else {
            return Err(WalletError::Transfer(
                "cannot prepare a self-funded build that prices its fee with real proofs; \
                 it would prove on every balancing round"
                    .into(),
            ));
        };

    Ok(PreparedTransfer {
        tx_info,
        tx,
        dust_batches,
        spent_unshielded_inputs: Vec::new(),
        spent_shielded_inputs: Vec::new(),
    })
}

/// Upper bound on fee-balancing rounds in [`pay_fees_no_validate`]. Each
/// round requests the candidate's full dust need, so in practice the loop
/// converges in 2-3 rounds; hitting the cap means the fee keeps growing
/// faster than the spends added to pay it.
const MAX_FEE_BALANCE_ITERATIONS: usize = 10;

/// Convergence bookkeeping for the fee-balancing loop in
/// [`pay_fees_no_validate`].
///
/// Each round rebuilds the candidate transaction from the original unpaid
/// `tx`, so dust spends never accumulate across rounds; the request passed
/// to `gather_dust_spends` must therefore cover the WHOLE need, not just
/// the latest gap. `record_shortfall` maintains that running total: the
/// dust provided in a round is always exactly the previous total
/// (`gather_dust_spends` errors unless the request is met in full), so
/// `total += current shortfall` makes the new total exactly the failed
/// candidate's full dust need. The loop is thus a fixpoint iteration on
/// the fee (request the last computed need, reprice the bigger tx,
/// repeat), which converges once adding spends stops growing the fee.
#[derive(Debug, Default)]
struct FeeBalanceTracker {
    /// Failed rounds so far.
    iterations: usize,
    /// Running total dust need; the request for the next round.
    missing_dust: u128,
    /// Fee (with margin) of the last unbalanced candidate, in specks.
    last_fee: Option<u128>,
}

impl FeeBalanceTracker {
    /// Dust to request from the funding wallets this round.
    fn request(&self) -> u128 {
        self.missing_dust
    }

    /// Record a round that came up short: `fee` is the candidate's
    /// computed fee with margin, `shortfall` the dust still missing.
    fn record_shortfall(&mut self, fee: u128, shortfall: u128) {
        self.iterations += 1;
        self.missing_dust = self.missing_dust.saturating_add(shortfall);
        self.last_fee = Some(fee);
    }

    fn into_error(self) -> WalletError {
        let fee = self
            .last_fee
            .map_or_else(|| "unknown".to_string(), |f| f.to_string());
        WalletError::Transfer(format!(
            "could not balance TX after {} iterations: last candidate needed {} specks of dust in total (last computed fee {} specks)",
            self.iterations, self.missing_dust, fee
        ))
    }
}

/// Converge the Dust fee with mock proofs, and stop before the real one.
///
/// Each round prices the candidate with `mock_prove`, which is exact for a
/// build carrying only builtin proofs (zswap spend/output, dust spend) because
/// their sizes are the constants the fee is computed from. A contract call's
/// proof size varies per circuit, and `mock_prove` refuses rather than
/// guessing, so such a build fails here instead of being underfunded.
///
/// Rounds are side-effect-free apart from `confirm_dust_spends` on the one
/// that converges: each round rebuilds the candidate from `tx`, so a later
/// round cannot double-spend what an earlier one selected.
fn balance_fees_with_mocks(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<(UnprovenTx, Vec<DustSpendBatch>), WalletError> {
    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let batches = gather_dust_spends(tx_info, tracker.request(), now)?;
        let flat_spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>> = batches
            .iter()
            .flat_map(|b| b.spends.iter().cloned())
            .collect();
        let mut paid_tx = tx.clone();
        apply_dust(
            tx_info,
            &mut paid_tx,
            &flat_spends,
            tx_info.rng.clone().split(),
            ttl,
            now,
        );

        let mock = paid_tx.mock_prove().map_err(transfer_err("mock_prove"))?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &mock)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        confirm_dust_spends(tx_info, &batches)?;
        return Ok((paid_tx, batches));
    }
    Err(tracker.into_error())
}

async fn pay_fees_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
    now: Timestamp,
    ttl: Timestamp,
) -> Result<BuiltTransaction, WalletError> {
    // Iterations are side-effect-free: `gather_dust_spends` only calls
    // `DustWallet::speculative_spend`, which takes `&self` and clones the
    // local state instead of writing it back, and each round rebuilds
    // `paid_tx` from the original `tx`. The only wallet mutation is
    // `mark_spent`, reached exclusively through `confirm_dust_spends` on
    // the success paths below; it must never move inside the loop, or a
    // retry after an unbalanced round would double-spend the dust the
    // failed round selected.
    if tx_info.mock_proofs_for_fees {
        let (paid_tx, dust_batches) = balance_fees_with_mocks(tx_info, tx, now, ttl)?;
        let finalized = prove_tx_no_validate(tx_info, paid_tx).await?;
        return Ok(BuiltTransaction {
            finalized,
            dust_batches,
        });
    }

    let mut tracker = FeeBalanceTracker::default();

    for _ in 0..MAX_FEE_BALANCE_ITERATIONS {
        let batches = gather_dust_spends(tx_info, tracker.request(), now)?;
        let flat_spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>> = batches
            .iter()
            .flat_map(|b| b.spends.iter().cloned())
            .collect();
        let mut paid_tx = tx.clone();
        apply_dust(
            tx_info,
            &mut paid_tx,
            &flat_spends,
            tx_info.rng.clone().split(),
            ttl,
            now,
        );

        let proven = prove_tx_no_validate(tx_info, paid_tx).await?;
        let (fee, shortfall) = compute_missing_dust(tx_info, &proven)?;
        if let Some(dust) = shortfall {
            tracker.record_shortfall(fee, dust);
            continue;
        }
        confirm_dust_spends(tx_info, &batches)?;
        return Ok(BuiltTransaction {
            finalized: proven,
            dust_batches: batches,
        });
    }
    Err(tracker.into_error())
}

async fn prove_tx_no_validate(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    tx: UnprovenTx,
) -> Result<FinalizedTx, WalletError> {
    let resolver = tx_info.context.resolver().await;
    let parameters = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?
        .parameters
        .clone();
    let mut rng = tx_info.rng.split();
    // `ProofProvider::prove` returns a bare transaction, so a backend that
    // fails has nowhere to report it but the unwind. Catch it here and hand
    // the caller a typed error instead of tearing down their task.
    let proven = std::panic::AssertUnwindSafe(tx_info.prover.prove(
        tx,
        rng.split(),
        &resolver,
        &parameters.cost_model.runtime_cost_model,
    ))
    .catch_unwind()
    .await
    .map_err(|payload| WalletError::Proving(panic_message(payload)))?;
    Ok(proven.seal(rng))
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

/// One funding-seed's dust contribution to a transaction.
///
/// `speculative_spend` returns both the spend records and the resulting
/// `DustLocalState`; the new helpers API requires that pair to be passed
/// together to `DustWallet::mark_spent`. We keep them grouped here so
/// callers (and the pending-reservations layer) can preserve the
/// invariant: the same `(spends, updated_state)` produced by a single
/// `speculative_spend` must be applied together.
#[derive(Clone)]
pub struct DustSpendBatch {
    pub seed: WalletSeed,
    pub spends: Vec<DustSpend<ProofPreimageMarker, DefaultDB>>,
    pub updated_state: Sp<DustLocalState<DefaultDB>, DefaultDB>,
}

fn gather_dust_spends(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    required_amount: u128,
    ctime: Timestamp,
) -> Result<Vec<DustSpendBatch>, WalletError> {
    let mut batches: Vec<DustSpendBatch> = Vec::new();
    let mut remaining = required_amount;
    let state = tx_info
        .context
        .ledger_state
        .lock()
        .map_err(|_| WalletError::Transfer("ledger state lock poisoned".into()))?;
    let params = &state.parameters.dust;
    let wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| WalletError::Transfer("wallets lock poisoned".into()))?;
    for seed in &tx_info.funding_seeds {
        if remaining == 0 {
            break;
        }
        // `get`, not `get_mut`: gathering must not mutate wallet state.
        // The fee-balancing retries in `pay_fees_no_validate` rely on
        // `speculative_spend` (`&self`) leaving the wallet untouched until
        // `confirm_dust_spends` applies the chosen batch via `mark_spent`.
        let wallet = wallets
            .get(seed)
            .ok_or_else(|| WalletError::Transfer("unrecognized wallet seed".into()))?;
        let (new_spends, updated_state) = wallet
            .dust
            .speculative_spend(remaining, ctime, params)
            .map_err(transfer_err("speculative_spend"))?;
        for spend in &new_spends {
            remaining = remaining.saturating_sub(spend.v_fee);
        }
        batches.push(DustSpendBatch {
            seed: seed.clone(),
            spends: new_spends,
            updated_state,
        });
    }
    if remaining > 0 {
        Err(WalletError::Transfer(format!(
            "insufficient DUST (trying to spend {required_amount}, need {remaining} more)"
        )))
    } else {
        Ok(batches)
    }
}

fn confirm_dust_spends(
    tx_info: &mut StandardTrasactionInfo<DefaultDB>,
    batches: &[DustSpendBatch],
) -> Result<(), WalletError> {
    let mut wallets = tx_info
        .context
        .wallets
        .lock()
        .map_err(|_| WalletError::Transfer("wallets lock poisoned".into()))?;
    for batch in batches {
        if let Some(wallet) = wallets.get_mut(&batch.seed) {
            wallet
                .dust
                .mark_spent(&batch.spends, batch.updated_state.clone());
        }
    }
    Ok(())
}

/// Price a candidate transaction. Returns the fee (with margin, in
/// specks) and, if the candidate doesn't balance, the dust shortfall.
fn compute_missing_dust(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    tx: &FinalizedTx,
) -> Result<(u128, Option<u128>), WalletError> {
    let fees = tx_info
        .context
        .with_ledger_state(|s| tx.fees_with_margin(&s.parameters, 3))
        .map_err(transfer_err("fees_with_margin"))?;
    let imbalances = tx.balance(Some(fees)).map_err(transfer_err("balance"))?;
    let dust_imbalance = imbalances
        .get(&(TokenType::Dust, Segment::Guaranteed.into()))
        .copied()
        .unwrap_or_default();
    if dust_imbalance < 0 {
        Ok((fees, Some(dust_imbalance.unsigned_abs())))
    } else {
        Ok((fees, None))
    }
}

fn apply_dust(
    tx_info: &StandardTrasactionInfo<DefaultDB>,
    tx: &mut UnprovenTx,
    spends: &[midnight_helpers::DustSpend<ProofPreimageMarker, DefaultDB>],
    mut rng: StdRng,
    ttl: Timestamp,
    now: Timestamp,
) {
    let Transaction::Standard(stx) = tx else {
        return;
    };

    if spends.is_empty() && tx_info.dust_registrations.is_empty() {
        return;
    }

    let segment_id: u16 = Segment::Fallible.into();
    let mut intent = match stx.intents.get(&segment_id) {
        Some(intent) => (*intent).clone(),
        None => Intent::empty(&mut rng, ttl),
    };
    let registrations = tx_info
        .dust_registrations
        .iter()
        .map(|registration| registration.build(&intent, &mut rng, segment_id))
        .collect::<Vec<_>>()
        .into();

    intent.dust_actions = Some(Sp::new(DustActions {
        spends: spends.to_vec().into(),
        registrations,
        ctime: now,
    }));
    stx.intents = stx.intents.insert(segment_id, intent);

    // Re-compute the binding randomness
    *tx = Transaction::new(
        stx.network_id.clone(),
        stx.intents.clone(),
        stx.guaranteed_coins.as_ref().map(|c| (**c).clone()),
        stx.fallible_coins
            .iter()
            .map(|sp| (*sp.0, (*sp.1).clone()))
            .collect(),
    );
}

/// Payload length of a shielded address: a 32-byte coin public key followed by
/// the 32-byte serialized encryption public key.
const SHIELDED_ADDRESS_DATA_LEN: usize = 64;

fn parse_wallet_address(s: &str, network: &Network) -> Result<WalletAddress, WalletError> {
    let addr = WalletAddress::from_str(s)
        .map_err(|e| WalletError::InvalidAddress(format!("bech32 decode: {e}")))?;
    check_address_network(&addr, network)?;
    Ok(addr)
}

/// Reject an address minted for a different network.
///
/// The HRP carries the network as a third `_`-separated segment, which mainnet
/// omits entirely (`mn_shield-addr1…`, per upstream `network_suffix`). The
/// upstream `TryFrom<&WalletAddress>` impls check the `mn` prefix and the
/// credential segment but never the network, so without this an address for
/// another chain decodes cleanly and builds a transfer to a key nobody here
/// controls.
fn check_address_network(addr: &WalletAddress, expected: &Network) -> Result<(), WalletError> {
    let hrp = addr.human_readable_part();
    // The network is everything after the credential segment, not just the
    // next segment: upstream appends `_{network_id}` verbatim, and a custom
    // network name may itself contain underscores.
    let actual = hrp.splitn(3, '_').nth(2);
    let want = match expected {
        Network::Mainnet => None,
        other => Some(other.as_str()),
    };
    if actual != want {
        return Err(WalletError::AddressNetworkMismatch {
            expected: expected.to_string(),
            actual: actual.unwrap_or(Network::Mainnet.as_str()).to_string(),
        });
    }
    Ok(())
}

fn parse_unshielded_recipient(
    s: &str,
    network: impl Into<Network>,
) -> Result<UnshieldedWallet, WalletError> {
    let addr = parse_wallet_address(s, &network.into())?;
    UnshieldedWallet::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not an unshielded address: {e:?}")))
}

/// Decode a `mn_shield-addr_*` bech32 string into a typed recipient suitable
/// for use as `OutputInfo::destination` when hand-building a shielded
/// [`OfferInfo`].
///
/// `network` is the network the address must belong to — normally the one the
/// spending wallet is synced to. An address for any other network is rejected
/// with [`WalletError::AddressNetworkMismatch`].
pub fn parse_shielded_recipient(
    s: &str,
    network: impl Into<Network>,
) -> Result<ShieldedWallet<DefaultDB>, WalletError> {
    let addr = parse_wallet_address(s, &network.into())?;
    // Upstream asserts this length rather than returning its `InvalidCoinKeyLen`
    // variant, so a truncated address would abort the process.
    let len = addr.data().len();
    if len != SHIELDED_ADDRESS_DATA_LEN {
        return Err(WalletError::InvalidAddress(format!(
            "shielded address payload is {len} bytes, expected {SHIELDED_ADDRESS_DATA_LEN}"
        )));
    }
    ShieldedWallet::<DefaultDB>::try_from(&addr)
        .map_err(|e| WalletError::InvalidAddress(format!("not a shielded address: {e:?}")))
}

#[cfg(test)]
mod tests {
    fn night_utxo(value: u128, ctime: Option<i64>, registered: bool) -> TrackedUtxo {
        TrackedUtxo {
            owner: "owner".into(),
            token_type: hex::encode(NIGHT.0.0),
            value,
            intent_hash: None,
            output_index: None,
            ctime,
            registered_for_dust_generation: Some(registered),
        }
    }

    /// Availability stands in for value against age, which is what the real
    /// one computes.
    fn by_value(u: &TrackedUtxo) -> u128 {
        u.value
    }

    #[test]
    fn the_registration_takes_the_utxo_carrying_the_most_dust() {
        let small = night_utxo(1, Some(100), false);
        let large = night_utxo(9, Some(100), false);
        let all = [&small, &large];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(9)
        );
    }

    #[test]
    fn a_registered_utxo_is_never_chosen() {
        let registered = night_utxo(9, Some(100), true);
        let free = night_utxo(1, Some(100), false);
        let all = [&registered, &free];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(1)
        );
    }

    #[test]
    fn every_utxo_registered_means_nothing_to_choose() {
        let a = night_utxo(9, Some(100), true);
        let all = [&a];
        assert!(choose_registration_input(&all, by_value).is_none());
    }

    /// A missing creation time forces the caller to guess an age, and a guess
    /// that runs ahead of the truth declares an allowance the ledger will not
    /// grant. So a guessed one loses to any real one, even a poorer one.
    #[test]
    fn a_utxo_without_a_creation_time_loses_to_one_that_has_it() {
        let guessed = night_utxo(9, None, false);
        let known = night_utxo(1, Some(100), false);
        let all = [&guessed, &known];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(1)
        );
    }

    #[test]
    fn a_guessed_creation_time_is_still_used_when_it_is_all_there_is() {
        let small = night_utxo(1, None, false);
        let large = night_utxo(9, None, false);
        let all = [&small, &large];
        assert_eq!(
            choose_registration_input(&all, by_value).map(|u| u.value),
            Some(9)
        );
    }

    use super::*;

    // Devnet and mainnet values, from the ledger's `INITIAL_DUST_PARAMETERS`.
    const RATIO: u64 = 5_000_000_000;
    const DECAY: u32 = 8267;

    fn avail(utxos: &[(u128, u64)], now_secs: u64) -> u128 {
        let utxos: Vec<_> = utxos
            .iter()
            .map(|&(v, c)| (v, Timestamp::from_secs(c)))
            .collect();
        generationless_fee_availability(&utxos, RATIO, DECAY, Timestamp::from_secs(now_secs))
    }

    /// Each UTXO ages from its own creation time. Summing a shared age over
    /// every UTXO is what the ledger refuses: it counts the older UTXO for
    /// more than the younger one.
    #[test]
    fn each_utxo_ages_from_its_own_ctime() {
        let now = 10_000;
        let old = avail(&[(1_000, 1_000)], now);
        let young = avail(&[(1_000, 9_000)], now);
        assert!(old > young, "the older UTXO must contribute more");

        let both = avail(&[(1_000, 1_000), (1_000, 9_000)], now);
        assert_eq!(both, old + young, "the total is the sum of the two ages");

        // Treating both as old, the way a single shared ctime would, declares
        // more than the ledger counts.
        let shared_age = avail(&[(1_000, 1_000), (1_000, 1_000)], now);
        assert!(shared_age > both);
    }

    /// Generation stops at the per-UTXO cap, so an ancient UTXO contributes
    /// `value * night_dust_ratio` and no more.
    #[test]
    fn generation_stops_at_the_cap() {
        let value = 1_000u128;
        let cap = value * RATIO as u128;
        // A UTXO reaches the cap after ceil(ratio / decay) seconds, about 7 days.
        let to_cap = (RATIO as u128).div_ceil(DECAY as u128) as u64;
        assert_eq!(to_cap, 604_815);
        assert!(avail(&[(value, 0)], to_cap - 1) < cap);
        assert_eq!(avail(&[(value, 0)], to_cap), cap);
        assert_eq!(avail(&[(value, 0)], 10_000_000), cap);
    }

    /// A UTXO created at or after `now` has no age, so it adds nothing.
    #[test]
    fn a_utxo_with_no_age_adds_nothing() {
        assert_eq!(avail(&[(1_000, 500)], 500), 0);
        assert_eq!(avail(&[(1_000, 900)], 500), 0);
    }

    /// A proof backend signals failure by panicking, because the ledger trait
    /// it implements returns a bare transaction. That unwind must not reach the
    /// caller: it becomes `WalletError::Proving`, carrying the backend's own
    /// message so the cause survives.
    #[tokio::test]
    async fn a_panicking_proof_backend_becomes_a_typed_error() {
        let caught = std::panic::AssertUnwindSafe(async {
            panic!("midnight-rs proving failed: no proving key for circuit `counter/increment`")
        })
        .catch_unwind()
        .await
        .map_err(|payload| WalletError::Proving(panic_message(payload)));

        let err = caught.expect_err("the panic should have been caught");
        let msg = err.to_string();
        assert!(msg.starts_with("proving failed:"), "got: {msg}");
        assert!(
            msg.contains("counter/increment"),
            "the backend's cause must survive the round trip, got: {msg}"
        );
    }

    #[test]
    fn panic_message_handles_both_payload_shapes() {
        assert_eq!(
            panic_message(Box::new("static str".to_string())),
            "static str"
        );
        assert_eq!(panic_message(Box::new("literal")), "literal");
        assert!(panic_message(Box::new(42u8)).contains("non-string payload"));
    }

    // The converging path of the fee-balancing loop (tracker is consulted
    // for the request, success returns without touching it) is exercised
    // end-to-end by the devnet integration tests (`build_shielded_transfer`
    // et al. in tests/integration.rs), which run the loop to convergence.

    #[test]
    fn tracker_requests_the_accumulated_total() {
        let mut t = FeeBalanceTracker::default();
        // Round 1 gathers nothing (request 0), so the shortfall is the
        // whole fee of the unpaid candidate.
        assert_eq!(t.request(), 0);
        t.record_shortfall(500, 500);
        assert_eq!(t.request(), 500);
        // Round 2 provided 500 but the bigger tx costs 620: shortfall is
        // the *current* gap (120), and the next request must cover the
        // whole need (620) because each round rebuilds the candidate tx
        // from scratch.
        t.record_shortfall(620, 120);
        assert_eq!(t.request(), 620);
    }

    #[test]
    fn non_convergence_error_reports_shortfall_iterations_and_fee() {
        let mut t = FeeBalanceTracker::default();
        for i in 0..MAX_FEE_BALANCE_ITERATIONS as u128 {
            t.record_shortfall(1_000 + i, 7);
        }
        let WalletError::Transfer(msg) = t.into_error() else {
            panic!("expected WalletError::Transfer");
        };
        assert!(
            msg.contains("10 iterations"),
            "missing iteration count: {msg}"
        );
        assert!(
            msg.contains("70 specks"),
            "missing accumulated dust need: {msg}"
        );
        assert!(msg.contains("1009"), "missing last fee: {msg}");
    }

    #[test]
    fn error_without_attempts_does_not_fabricate_a_fee() {
        let WalletError::Transfer(msg) = FeeBalanceTracker::default().into_error() else {
            panic!("expected WalletError::Transfer");
        };
        assert!(msg.contains("0 iterations"), "{msg}");
        assert!(msg.contains("unknown"), "{msg}");
    }

    /// The strategy has to reach the selector, not just be stored. Both
    /// orderings cover the amount, but they reach for opposite ends of the
    /// wallet: largest-first takes the fewest coins, smallest-first takes the
    /// most and so absorbs the small ones.
    #[test]
    fn coin_selection_strategy_picks_opposite_ends() {
        let default_strategy = CoinSelectionStrategy::default();
        assert!(
            matches!(default_strategy, CoinSelectionStrategy::LargestFirst),
            "the default must stay LargestFirst: every shielded input carries \
             its own proof, so the default optimises for the fewest inputs"
        );
        assert!(!matches!(
            CoinSelectionStrategy::SmallestFirst,
            CoinSelectionStrategy::LargestFirst
        ));
    }
}
