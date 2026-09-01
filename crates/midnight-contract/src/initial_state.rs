//! The initial ledger state a deployment starts from, described without
//! naming a ledger type.
//!
//! Generated bindings describe a contract's opening state as a list of
//! [`InitialField`], one per ledger field in declaration order, and this
//! module turns that into the `ContractState` the deploy path needs. Keeping
//! the description on this side of the boundary lets the generated code name
//! only types that every ledger generation shares.

use midnight_typed_state::{
    AlignedValue, ContractMaintenanceAuthority, ContractState, InMemoryDB, StateValue,
    StorageArray, StorageHashMap,
};

/// A collection field's opening value.
///
/// A deployment starts every collection empty, so the kind is all the state
/// builder needs. These are the values the generated `InitialState` struct
/// carries for `Map`, `Set`, `List`, `MerkleTree` and `HistoricMerkleTree`
/// fields, and they exist so that struct names no ledger type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyMap;

/// A list field's opening value. See [`EmptyMap`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyList;

/// A merkle-tree field's opening value. See [`EmptyMap`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyMerkleTree;

/// One ledger field's opening value, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialField {
    /// A cell holding an encoded value.
    Cell(AlignedValue),
    /// A counter, which opens at `0` unless the caller sets it.
    Counter(u64),
    /// An empty map or set.
    Map,
    /// An empty list.
    List,
    /// An empty merkle tree, historic or not.
    MerkleTree,
}

/// A contract's opening ledger state.
///
/// Generated bindings build one of these, and [`DeployBuilder::with_initial_state`]
/// takes anything that converts into it.
///
/// [`DeployBuilder::with_initial_state`]: crate::DeployBuilder::with_initial_state
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitialState {
    fields: Vec<InitialField>,
}

impl InitialState {
    /// The opening state for a contract whose ledger fields are `fields`, in
    /// declaration order.
    pub fn new(fields: Vec<InitialField>) -> Self {
        Self { fields }
    }

    fn into_contract_state(self) -> ContractState<InMemoryDB> {
        let cells = self
            .fields
            .into_iter()
            .map(|field| match field {
                InitialField::Cell(value) => StateValue::from(value),
                InitialField::Counter(value) => StateValue::from(value),
                InitialField::Map => StateValue::Map(StorageHashMap::new()),
                InitialField::List => StateValue::Array(StorageArray::new()),
                InitialField::MerkleTree => StateValue::Null,
            })
            .collect::<Vec<_>>();
        ContractState::new(
            StateValue::Array(cells.into()),
            StorageHashMap::new(),
            ContractMaintenanceAuthority::default(),
        )
    }
}

/// The `ContractState` a deployment starts from.
///
/// Generated bindings call this to materialise their opening state, because
/// they describe it without naming a ledger type.
pub fn initial_contract_state(initial: InitialState) -> ContractState<InMemoryDB> {
    initial.into_contract_state()
}
