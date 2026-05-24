use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// GraphQL envelope
// ---------------------------------------------------------------------------

/// Wrapper for GraphQL response envelope.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct GraphQLResponse<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Option<Vec<GraphQLError>>,
}

/// A single GraphQL error from the response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct GraphQLError {
    pub message: String,
}

impl std::fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub hash: String,
    pub height: i64,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub transactions: Option<Vec<Transaction>>,
    #[serde(default)]
    pub ledger_parameters: Option<String>,
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

/// A Midnight transaction (regular or system), discriminated by `__typename`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "__typename")]
pub enum Transaction {
    RegularTransaction(RegularTransaction),
    SystemTransaction(SystemTransaction),
}

impl Transaction {
    pub fn id(&self) -> i64 {
        match self {
            Self::RegularTransaction(tx) => tx.id,
            Self::SystemTransaction(tx) => tx.id,
        }
    }

    pub fn hash(&self) -> &str {
        match self {
            Self::RegularTransaction(tx) => &tx.hash,
            Self::SystemTransaction(tx) => &tx.hash,
        }
    }

    pub fn block(&self) -> Option<&Block> {
        match self {
            Self::RegularTransaction(tx) => tx.block.as_ref(),
            Self::SystemTransaction(tx) => tx.block.as_ref(),
        }
    }

    pub fn contract_actions(&self) -> &[ContractAction] {
        match self {
            Self::RegularTransaction(tx) => tx.contract_actions.as_deref().unwrap_or_default(),
            Self::SystemTransaction(tx) => tx.contract_actions.as_deref().unwrap_or_default(),
        }
    }

    /// The chain's [`TransactionResult`] for this transaction, if the
    /// indexer has surfaced one yet. Only present on [`RegularTransaction`]
    /// (system transactions have no result status).
    pub fn transaction_result(&self) -> Option<&TransactionResult> {
        match self {
            Self::RegularTransaction(tx) => tx.transaction_result.as_ref(),
            Self::SystemTransaction(_) => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegularTransaction {
    pub id: i64,
    pub hash: String,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub raw: Option<String>,
    #[serde(default)]
    pub identifiers: Option<Vec<String>>,
    #[serde(default)]
    pub merkle_tree_root: Option<String>,
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default)]
    pub end_index: Option<i64>,
    #[serde(default)]
    pub fees: Option<TransactionFees>,
    #[serde(default)]
    pub transaction_result: Option<TransactionResult>,
    #[serde(default)]
    pub block: Option<Block>,
    #[serde(default)]
    pub contract_actions: Option<Vec<ContractAction>>,
    #[serde(default)]
    pub unshielded_created_outputs: Option<Vec<UnshieldedUtxo>>,
    #[serde(default)]
    pub unshielded_spent_outputs: Option<Vec<UnshieldedUtxo>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemTransaction {
    pub id: i64,
    pub hash: String,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub raw: Option<String>,
    #[serde(default)]
    pub block: Option<Block>,
    #[serde(default)]
    pub contract_actions: Option<Vec<ContractAction>>,
    #[serde(default)]
    pub unshielded_created_outputs: Option<Vec<UnshieldedUtxo>>,
    #[serde(default)]
    pub unshielded_spent_outputs: Option<Vec<UnshieldedUtxo>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransactionFees {
    pub paid_fees: String,
    pub estimated_fees: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransactionResult {
    pub status: TransactionResultStatus,
    #[serde(default)]
    pub segments: Option<Vec<Segment>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionResultStatus {
    Success,
    PartialSuccess,
    Failure,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Segment {
    pub id: i64,
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Contract actions
// ---------------------------------------------------------------------------

/// A contract action (deploy, call, or update), discriminated by `__typename`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "__typename")]
pub enum ContractAction {
    ContractDeploy(ContractDeploy),
    ContractCall(ContractCall),
    ContractUpdate(ContractUpdate),
}

impl ContractAction {
    pub fn address(&self) -> &str {
        match self {
            Self::ContractDeploy(a) => &a.address,
            Self::ContractCall(a) => &a.address,
            Self::ContractUpdate(a) => &a.address,
        }
    }

    pub fn state(&self) -> &str {
        match self {
            Self::ContractDeploy(a) => &a.state,
            Self::ContractCall(a) => &a.state,
            Self::ContractUpdate(a) => &a.state,
        }
    }

    pub fn zswap_state(&self) -> Option<&str> {
        match self {
            Self::ContractDeploy(a) => a.zswap_state.as_deref(),
            Self::ContractCall(a) => a.zswap_state.as_deref(),
            Self::ContractUpdate(a) => a.zswap_state.as_deref(),
        }
    }

    pub fn unshielded_balances(&self) -> &[ContractBalance] {
        match self {
            Self::ContractDeploy(a) => a.unshielded_balances.as_deref().unwrap_or_default(),
            Self::ContractCall(a) => a.unshielded_balances.as_deref().unwrap_or_default(),
            Self::ContractUpdate(a) => a.unshielded_balances.as_deref().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContractDeploy {
    pub address: String,
    pub state: String,
    #[serde(default)]
    pub zswap_state: Option<String>,
    #[serde(default)]
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    #[serde(default)]
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContractCall {
    pub address: String,
    pub state: String,
    #[serde(default)]
    pub zswap_state: Option<String>,
    #[serde(default)]
    pub entry_point: Option<String>,
    #[serde(default)]
    pub deploy: Option<Box<ContractDeploy>>,
    #[serde(default)]
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    #[serde(default)]
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContractUpdate {
    pub address: String,
    pub state: String,
    #[serde(default)]
    pub zswap_state: Option<String>,
    #[serde(default)]
    pub unshielded_balances: Option<Vec<ContractBalance>>,
    #[serde(default)]
    pub transaction: Option<Box<Transaction>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContractBalance {
    pub token_type: String,
    pub amount: String,
}

// ---------------------------------------------------------------------------
// Unshielded UTXOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnshieldedUtxo {
    pub owner: String,
    pub token_type: String,
    pub value: String,
    #[serde(default)]
    pub intent_hash: Option<String>,
    #[serde(default)]
    pub output_index: Option<i64>,
    #[serde(default)]
    pub ctime: Option<i64>,
    #[serde(default)]
    pub registered_for_dust_generation: Option<bool>,
}

// ---------------------------------------------------------------------------
// Query offset types (mirror GraphQL schema inputs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HeightOffset {
    pub height: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HashOffset {
    pub hash: String,
}

/// Offset for block queries: fetch by height or hash.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum BlockOffset {
    Height { height: i64 },
    Hash { hash: String },
}

impl BlockOffset {
    pub fn height(h: i64) -> Self {
        Self::Height { height: h }
    }

    pub fn hash(h: impl Into<String>) -> Self {
        Self::Hash { hash: h.into() }
    }
}

/// Offset for contract-action queries: by block height, block hash, or tx hash.
///
/// Construct via the helper methods [`block_height`](Self::block_height),
/// [`block_hash`](Self::block_hash), and [`tx_hash`](Self::tx_hash).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(private_interfaces)]
pub enum ContractActionOffset {
    #[serde(rename_all = "camelCase")]
    BlockHeight { block_offset: HeightOffset },
    #[serde(rename_all = "camelCase")]
    BlockHash { block_offset: HashOffset },
    #[serde(rename_all = "camelCase")]
    TxHash { transaction_offset: HashOffset },
}

impl ContractActionOffset {
    pub fn block_height(h: i64) -> Self {
        Self::BlockHeight {
            block_offset: HeightOffset { height: h },
        }
    }

    pub fn block_hash(h: impl Into<String>) -> Self {
        Self::BlockHash {
            block_offset: HashOffset { hash: h.into() },
        }
    }

    pub fn tx_hash(h: impl Into<String>) -> Self {
        Self::TxHash {
            transaction_offset: HashOffset { hash: h.into() },
        }
    }
}

/// Offset for transaction queries: by hash or identifier.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TransactionOffset {
    Hash { hash: String },
    Identifier { identifier: String },
}

impl TransactionOffset {
    pub fn hash(h: impl Into<String>) -> Self {
        Self::Hash { hash: h.into() }
    }

    pub fn identifier(id: impl Into<String>) -> Self {
        Self::Identifier {
            identifier: id.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Query variable structs (used internally by IndexerClient)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Default)]
pub(crate) struct BlockQueryVars {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<BlockOffset>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContractActionQueryVars {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<ContractActionOffset>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TransactionsQueryVars {
    pub offset: TransactionOffset,
}

// ---------------------------------------------------------------------------
// Query-specific response wrappers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct BlockQueryData {
    pub block: Option<Block>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContractActionQueryData {
    pub contract_action: Option<ContractAction>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TransactionsQueryData {
    pub transactions: Vec<Transaction>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_block() {
        let json = r#"{
            "hash": "abc123",
            "height": 42,
            "protocolVersion": 1,
            "timestamp": 1700000000,
            "author": "node-1",
            "ledgerParameters": null
        }"#;

        let block: Block = serde_json::from_str(json).unwrap();
        assert_eq!(block.hash, "abc123");
        assert_eq!(block.height, 42);
        assert_eq!(block.protocol_version, Some(1));
        assert_eq!(block.timestamp, Some(1700000000));
        assert_eq!(block.author, Some("node-1".to_string()));
        assert!(block.transactions.is_none());
    }

    #[test]
    fn deserialize_regular_transaction() {
        let json = r#"{
            "__typename": "RegularTransaction",
            "id": 1,
            "hash": "tx_hash_1",
            "protocolVersion": 1,
            "identifiers": ["id1", "id2"],
            "transactionResult": {
                "status": "SUCCESS",
                "segments": [{"id": 0, "success": true}]
            }
        }"#;
        let tx: Transaction = serde_json::from_str(json).unwrap();
        assert_eq!(tx.id(), 1);
        assert_eq!(tx.hash(), "tx_hash_1");
        match &tx {
            Transaction::RegularTransaction(rt) => {
                assert_eq!(rt.identifiers.as_ref().unwrap().len(), 2);
                assert_eq!(
                    rt.transaction_result.as_ref().unwrap().status,
                    TransactionResultStatus::Success
                );
            }
            _ => panic!("expected RegularTransaction"),
        }
    }

    #[test]
    fn deserialize_system_transaction() {
        let json = r#"{
            "__typename": "SystemTransaction",
            "id": 99,
            "hash": "systxhash"
        }"#;

        let tx: Transaction = serde_json::from_str(json).unwrap();
        assert!(matches!(tx, Transaction::SystemTransaction(_)));
        assert_eq!(tx.id(), 99);
        assert_eq!(tx.hash(), "systxhash");
    }

    #[test]
    fn deserialize_contract_call_action() {
        let json = r#"{
            "__typename": "ContractCall",
            "address": "contract_addr",
            "state": "deadbeef",
            "entryPoint": "withdraw",
            "zswapState": "cafe"
        }"#;
        let action: ContractAction = serde_json::from_str(json).unwrap();
        assert_eq!(action.address(), "contract_addr");
        assert_eq!(action.state(), "deadbeef");
        assert_eq!(action.zswap_state(), Some("cafe"));
        match &action {
            ContractAction::ContractCall(c) => {
                assert_eq!(c.entry_point.as_deref(), Some("withdraw"));
            }
            _ => panic!("expected ContractCall"),
        }
    }

    #[test]
    fn deserialize_contract_deploy_action() {
        let json = r#"{
            "__typename": "ContractDeploy",
            "address": "deployaddr",
            "state": "deploystate",
            "unshieldedBalances": [
                {"tokenType": "tDUST", "amount": "1000"}
            ]
        }"#;

        let action: ContractAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, ContractAction::ContractDeploy(_)));

        let balances = action.unshielded_balances();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].token_type, "tDUST");
        assert_eq!(balances[0].amount, "1000");
    }

    #[test]
    fn deserialize_graphql_error() {
        let json = r#"{"message": "field not found"}"#;
        let err: GraphQLError = serde_json::from_str(json).unwrap();
        assert_eq!(err.message, "field not found");
        assert_eq!(err.to_string(), "field not found");
    }

    #[test]
    fn transaction_accessor_methods() {
        let tx = RegularTransaction {
            id: 7,
            hash: "myhash".to_string(),
            protocol_version: None,
            raw: None,
            identifiers: None,
            merkle_tree_root: None,
            start_index: None,
            end_index: None,
            fees: None,
            transaction_result: None,
            block: Some(Block {
                hash: "blockhash".to_string(),
                height: 10,
                protocol_version: None,
                timestamp: None,
                author: None,
                transactions: None,
                ledger_parameters: None,
            }),
            contract_actions: Some(vec![ContractAction::ContractUpdate(ContractUpdate {
                address: "ca1".to_string(),
                state: "st1".to_string(),
                zswap_state: None,
                unshielded_balances: None,
                transaction: None,
            })]),
            unshielded_created_outputs: None,
            unshielded_spent_outputs: None,
        };

        let wrapped = Transaction::RegularTransaction(tx);

        assert_eq!(wrapped.id(), 7);
        assert_eq!(wrapped.hash(), "myhash");
        assert_eq!(wrapped.block().map(|b| b.height), Some(10));
        assert_eq!(wrapped.contract_actions().len(), 1);
        assert_eq!(wrapped.contract_actions()[0].address(), "ca1");
    }
}
