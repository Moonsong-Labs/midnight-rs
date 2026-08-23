//! Names for the error codes a node returns when it refuses a transaction.
//!
//! A refusal arrives as substrate's `InvalidTransaction::Custom(u8)`, rendered
//! into prose by the RPC layer. The byte's meaning belongs to the node's
//! runtime, so no metadata lookup resolves it and the table below is
//! transcribed from `ledger/src/versions/common/types.rs` in midnight-node at
//! tag **node-0.22.1**, which is the version `devnet/docker-compose.yml` runs.
//!
//! Codes are not stable across node releases. midnight-node keeps a
//! `RETIRED_U8_ERROR_CODES` list precisely because assignments have been
//! withdrawn, and `168` is on it upstream while the node we pin still emits
//! it. So treat a name as a reading aid, never as a protocol constant, and
//! re-transcribe when the pinned node image moves.

/// `(code, name)` as midnight-node assigns them, sorted by code.
static CODES: &[(u8, &str)] = &[
    (0, "NetworkId"),
    (1, "Transaction"),
    (2, "DeserializationLedgerState"),
    (3, "DeserializationContractAddress"),
    (4, "PublicKey"),
    (5, "VersionedArenaKey"),
    (6, "UserAddress"),
    (7, "TypedArenaKey"),
    (8, "SystemTransaction"),
    (9, "DustPublicKey"),
    (10, "CNightGeneratesDustActionType"),
    (11, "CNightGeneratesDustEvent"),
    (50, "TransactionIdentifier"),
    (51, "SerializationLedgerState"),
    (52, "LedgerParameters"),
    (53, "SerializationContractAddress"),
    (54, "ContractState"),
    (55, "ContractStateToJson"),
    (56, "ZswapState"),
    (57, "UnknownType"),
    (58, "MerkleTreeDigest"),
    (59, "VersionedArenaKey"),
    (60, "TypedArenaKey"),
    (61, "CNightGeneratesDustEvent"),
    (62, "SystemTransaction"),
    (63, "ArenaHash"),
    (100, "EffectsMismatch"),
    (101, "ContractAlreadyDeployed"),
    (102, "ContractNotPresent"),
    (103, "Zswap"),
    (104, "Transcript"),
    (105, "InsufficientClaimable"),
    (106, "VerifierKeyNotFound"),
    (107, "VerifierKeyAlreadyPresent"),
    (108, "ReplayCounterMismatch"),
    (109, "UnknownError"),
    (110, "VerifierKeyNotSet"),
    (111, "TransactionTooLarge"),
    (112, "VerifierKeyTooLarge"),
    (113, "VerifierKeyNotPresent"),
    (114, "ContractNotPresent"),
    (115, "InvalidProof"),
    (116, "BindingCommitmentOpeningInvalid"),
    (117, "NotNormalized"),
    (118, "FallibleWithoutCheckpoint"),
    (119, "ClaimReceiveFailed"),
    (120, "ClaimSpendFailed"),
    (121, "ClaimNullifierFailed"),
    (122, "ClaimCallFailed"),
    (123, "InvalidSchnorrProof"),
    (124, "UnclaimedCoinCom"),
    (125, "UnclaimedNullifier"),
    (126, "Unbalanced"),
    (127, "Zswap"),
    (128, "BuiltinDecode"),
    (129, "GuaranteedLimit"),
    (130, "MergingContracts"),
    (131, "CantMergeTypes"),
    (132, "ClaimOverflow"),
    (133, "ClaimCoinMismatch"),
    (134, "KeyNotInCommittee"),
    (135, "InvalidCommitteeSignature"),
    (136, "ThresholdMissed"),
    (137, "TooManyZswapEntries"),
    (138, "BalanceCheckOverspend"),
    (139, "UnknownError"),
    (150, "LedgerCacheError"),
    (151, "NoLedgerState"),
    (152, "LedgerStateScaleDecodingError"),
    (153, "ContractCallCostError"),
    (154, "BlockLimitExceededError"),
    (155, "FeeCalculationError"),
    (165, "GetTransactionContextError"),
    (166, "InvalidNetworkId"),
    (167, "IllegallyDeclaredGuaranteed"),
    (168, "FeeCalculation"),
    (169, "InvalidDustRegistrationSignature"),
    (170, "InvalidDustSpendProof"),
    (171, "OutOfDustValidityWindow"),
    (172, "MultipleDustRegistrationsForKey"),
    (173, "InsufficientDustForRegistrationFee"),
    (174, "MalformedContractDeploy"),
    (175, "IntentSignatureVerificationFailure"),
    (176, "IntentSignatureKeyMismatch"),
    (177, "IntentSegmentIdCollision"),
    (178, "IntentAtGuaranteedSegmentId"),
    (179, "UnsupportedProofVersion"),
    (180, "GuaranteedTranscriptVersion"),
    (181, "FallibleTranscriptVersion"),
    (182, "TransactionApplicationError"),
    (183, "BalanceCheckOutOfBounds"),
    (184, "BalanceCheckConversionFailure"),
    (185, "PedersenCheckFailure"),
    (186, "EffectsCheckFailure"),
    (187, "DisjointCheckFailure"),
    (188, "SequencingCheckFailure"),
    (189, "InputsNotSorted"),
    (190, "OutputsNotSorted"),
    (191, "DuplicateInputs"),
    (192, "InputsSignaturesLengthMismatch"),
    (193, "ReplayProtectionViolation"),
    (194, "BalanceCheckOutOfBounds"),
    (195, "InputNotInUtxos"),
    (196, "DustDoubleSpend"),
    (197, "DustDeregistrationNotRegistered"),
    (198, "GenerationInfoAlreadyPresent"),
    (199, "InvariantViolation"),
    (200, "RewardTooSmall"),
    (201, "IllegalPayout"),
    (202, "InsufficientTreasuryFunds"),
    (203, "CommitmentAlreadyPresent"),
    (204, "UnknownError"),
    (205, "ReplayProtectionFailure"),
    (206, "IllegalReserveDistribution"),
    (207, "GenerationInfoAlreadyPresent"),
    (208, "InvalidBasisPoints"),
    (209, "InvariantViolation"),
    (210, "TreasuryDisabled"),
    (255, "HostApiError"),
];

/// The name midnight-node gives `code`, or `None` when this build does not
/// know it. An unknown code is normal after a node bump; the caller still has
/// the number and the node's own message.
pub fn name_of(code: u8) -> Option<&'static str> {
    CODES
        .binary_search_by_key(&code, |(c, _)| *c)
        .ok()
        .map(|i| CODES[i].1)
}

/// The `Custom(u8)` code carried by a node's refusal message, when it has one.
///
/// Read out of the text because that is the only form available: subxt's
/// `TransactionStatus::Invalid` carries a human-readable `message` and nothing
/// else, so the watch stream has already discarded the typed
/// `InvalidTransaction::Custom(u8)` by the time we see it. There is no typed
/// value on this path to reach for instead.
/// A message that does not carry one yields `None`, which is not an error:
/// plenty of refusals (a bad nonce, a stale mortality) never reach the
/// ledger's own error mapping.
pub fn code_in(message: &str) -> Option<u8> {
    let tail = message.rsplit_once("custom error")?.1;
    let digits: String = tail
        .trim_start_matches([':', ' '])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_has_no_repeats() {
        for pair in CODES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "codes must be sorted and distinct: {} then {}",
                pair[0].0,
                pair[1].0
            );
        }
    }

    #[test]
    fn codes_this_repo_has_observed_keep_their_names() {
        // Every one of these was seen coming back from a devnet node, so a
        // re-transcription that moves them is a mistake rather than a bump.
        assert_eq!(name_of(168), Some("FeeCalculation"));
        assert_eq!(name_of(171), Some("OutOfDustValidityWindow"));
        assert_eq!(name_of(173), Some("InsufficientDustForRegistrationFee"));
        assert_eq!(name_of(195), Some("InputNotInUtxos"));
        assert_eq!(name_of(196), Some("DustDoubleSpend"));
    }

    #[test]
    fn a_code_outside_a_byte_is_not_a_custom_code() {
        // `InvalidTransaction::Custom` carries a u8, so anything wider came
        // from somewhere else and must not be named as if it had.
        assert_eq!(code_in("custom error: 300"), None);
        assert_eq!(code_in("custom error: 255"), Some(255));
    }

    #[test]
    fn only_digits_are_taken_and_a_missing_number_is_not_one() {
        assert_eq!(code_in("custom error: 168, and more text"), Some(168));
        assert_eq!(code_in("custom error: -5"), None);
        assert_eq!(code_in("custom error:"), None);
        // The last occurrence wins, which is the one the node appended.
        assert_eq!(code_in("custom error: 1 then custom error: 173"), Some(173));
    }

    #[test]
    fn a_code_is_read_out_of_the_node_message() {
        assert_eq!(
            code_in("Invalid transaction with custom error: 168"),
            Some(168)
        );
        assert_eq!(
            code_in("Invalid transaction with custom error:173"),
            Some(173)
        );
        assert_eq!(code_in("Transaction has a bad signature"), None);
        assert_eq!(code_in("custom error: not-a-number"), None);
    }
}
