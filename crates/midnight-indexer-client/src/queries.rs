/// Fetch a block by optional offset (hash or height). Returns latest if no offset.
pub const BLOCK_QUERY: &str = r#"
    query BlockQuery($offset: BlockOffset) {
        block(offset: $offset) {
            hash
            height
            protocolVersion
            timestamp
            author
            ledgerParameters
        }
    }
"#;

/// Fetch a block with its transactions included.
pub const BLOCK_WITH_TRANSACTIONS_QUERY: &str = r#"
    query BlockWithTransactionsQuery($offset: BlockOffset) {
        block(offset: $offset) {
            hash
            height
            protocolVersion
            timestamp
            author
            ledgerParameters
            transactions {
                __typename
                ... on RegularTransaction {
                    id
                    hash
                    protocolVersion
                    identifiers
                    transactionResult {
                        status
                        segments { id success }
                    }
                }
                ... on SystemTransaction {
                    id
                    hash
                    protocolVersion
                }
            }
        }
    }
"#;

/// Fetch the contract state (hex-encoded) at a given address with optional offset.
pub const CONTRACT_STATE_QUERY: &str = r#"
    query ContractStateQuery($address: HexEncoded!, $offset: ContractActionOffset) {
        contractAction(address: $address, offset: $offset) {
            __typename
            address
            state
        }
    }
"#;

/// Fetch the full contract action (state, chain state, balances) at a given address.
pub const CONTRACT_ACTION_QUERY: &str = r#"
    query ContractActionQuery($address: HexEncoded!, $offset: ContractActionOffset) {
        contractAction(address: $address, offset: $offset) {
            __typename
            address
            state
            zswapState
            unshieldedBalances {
                tokenType
                amount
            }
            ... on ContractCall {
                entryPoint
                deploy {
                    address
                    state
                    zswapState
                    unshieldedBalances {
                        tokenType
                        amount
                    }
                }
            }
        }
    }
"#;

/// Fetch the block height of the latest transaction touching a contract.
pub const LATEST_CONTRACT_BLOCK_HEIGHT_QUERY: &str = r#"
    query LatestContractBlockHeightQuery($address: HexEncoded!) {
        contractAction(address: $address) {
            __typename
            transaction {
                __typename
                block {
                    height
                }
            }
        }
    }
"#;

/// Fetch transactions by hash or identifier.
pub const TRANSACTIONS_QUERY: &str = r#"
    query TransactionsQuery($offset: TransactionOffset!) {
        transactions(offset: $offset) {
            __typename
            ... on RegularTransaction {
                id
                hash
                protocolVersion
                raw
                identifiers
                merkleTreeRoot
                startIndex
                endIndex
                fees {
                    paidFees
                    estimatedFees
                }
                transactionResult {
                    status
                    segments { id success }
                }
                block {
                    hash
                    height
                    timestamp
                    author
                }
                contractActions {
                    __typename
                    address
                    state
                    ... on ContractCall {
                        entryPoint
                    }
                }
                unshieldedCreatedOutputs {
                    owner
                    intentHash
                    tokenType
                    value
                }
                unshieldedSpentOutputs {
                    owner
                    intentHash
                    tokenType
                    value
                }
            }
            ... on SystemTransaction {
                id
                hash
                protocolVersion
                raw
                block {
                    hash
                    height
                    timestamp
                    author
                }
                contractActions {
                    __typename
                    address
                    state
                }
            }
        }
    }
"#;
