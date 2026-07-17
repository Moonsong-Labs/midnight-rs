use midnight_base_crypto::fab::{AlignedValue, InvalidBuiltinDecode, ValueSlice};
use midnight_onchain_state::state::StateValue;
use midnight_storage::db::InMemoryDB;
use midnight_storage::storage::{Array, HashMap};
use midnight_transient_crypto::merkle_tree::{MerkleTree, MerkleTreeDigest};
use std::marker::PhantomData;

use crate::StateError;

/// Typed accessor for a Midnight contract map field.
///
/// Maps are stored as `HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>`.
/// The accessor provides typed lookup and iteration by converting keys and values
/// through `Into<AlignedValue>` / `TryFrom<&ValueSlice>`.
pub struct MapAccessor<'a, K, V> {
    map: &'a HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>,
    _phantom: PhantomData<(K, V)>,
}

impl<'a, K, V> MapAccessor<'a, K, V> {
    /// Creates a new accessor wrapping the given map reference.
    pub fn new(map: &'a HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>) -> Self {
        Self {
            map,
            _phantom: PhantomData,
        }
    }

    /// Returns the number of entries in the map.
    pub fn size(&self) -> usize {
        self.map.size()
    }

    /// Returns `true` if the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl<K: Into<AlignedValue>, V> MapAccessor<'_, K, V> {
    /// Returns `true` if the map contains the given key.
    pub fn contains_key(&self, key: K) -> bool {
        self.map.contains_key(&key.into())
    }
}

impl<K, V> MapAccessor<'_, K, V>
where
    K: Into<AlignedValue>,
    for<'vs> V: TryFrom<&'vs ValueSlice, Error = InvalidBuiltinDecode>,
{
    /// Looks up a key in the map and converts the associated cell value.
    ///
    /// Returns `None` if the key is not present. Returns `Some(Err(..))` if the
    /// key is present but the value cannot be converted.
    pub fn get(&self, key: K) -> Option<Result<V, StateError>> {
        let sp = self.map.get(&key.into())?;
        let sv: &StateValue<InMemoryDB> = &sp;
        let av = match crate::nav::cell_value(sv) {
            Ok(av) => av,
            Err(e) => return Some(Err(e)),
        };
        Some(V::try_from(&*av.value).map_err(StateError::Conversion))
    }
}

impl<K, V> MapAccessor<'_, K, V>
where
    for<'vs> K: TryFrom<&'vs ValueSlice, Error = InvalidBuiltinDecode>,
    for<'vs> V: TryFrom<&'vs ValueSlice, Error = InvalidBuiltinDecode>,
{
    /// Iterates over all key-value pairs, converting both keys and values.
    pub fn iter(&self) -> impl Iterator<Item = Result<(K, V), StateError>> + '_ {
        self.map.iter().map(|pair| {
            let (key_sp, val_sp) = &*pair;
            let key_av: &AlignedValue = key_sp;
            let k = K::try_from(&*key_av.value).map_err(StateError::Conversion)?;

            let sv: &StateValue<InMemoryDB> = val_sp;
            let av = crate::nav::cell_value(sv)?;
            let v = V::try_from(&*av.value).map_err(StateError::Conversion)?;

            Ok((k, v))
        })
    }
}

/// Typed accessor for a Midnight contract set field.
///
/// Sets are stored as `HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>`
/// where every value is `StateValue::Null`. The accessor provides typed membership
/// checks and iteration over the key set.
pub struct SetAccessor<'a, T> {
    map: &'a HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>,
    _phantom: PhantomData<T>,
}

impl<'a, T> SetAccessor<'a, T> {
    /// Creates a new accessor wrapping the given map reference.
    pub fn new(map: &'a HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>) -> Self {
        Self {
            map,
            _phantom: PhantomData,
        }
    }

    /// Returns the number of elements in the set.
    pub fn size(&self) -> usize {
        self.map.size()
    }

    /// Returns `true` if the set has no elements.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl<T: Into<AlignedValue>> SetAccessor<'_, T> {
    /// Returns `true` if the set contains the given element.
    pub fn contains(&self, key: T) -> bool {
        self.map.contains_key(&key.into())
    }
}

impl<T> SetAccessor<'_, T>
where
    for<'vs> T: TryFrom<&'vs ValueSlice, Error = InvalidBuiltinDecode>,
{
    /// Iterates over all elements in the set, converting each key.
    pub fn iter(&self) -> impl Iterator<Item = Result<T, StateError>> + '_ {
        self.map
            .keys()
            .map(|key_av| T::try_from(&*key_av.value).map_err(StateError::Conversion))
    }
}

/// Typed accessor for a Midnight contract list field.
///
/// Lists are stored as `Array<StateValue<InMemoryDB>, InMemoryDB>` where each
/// element is a `StateValue::Cell(AlignedValue)` containing the serialized value.
/// The accessor provides typed indexing and iteration by converting elements
/// through `TryFrom<&ValueSlice>`.
pub struct ListAccessor<'a, T> {
    array: &'a Array<StateValue<InMemoryDB>, InMemoryDB>,
    _phantom: PhantomData<T>,
}

impl<T> std::fmt::Debug for ListAccessor<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListAccessor")
            .field("len", &self.array.len())
            .finish()
    }
}

impl<'a, T> ListAccessor<'a, T> {
    /// Creates a new accessor wrapping the given array reference.
    pub fn new(array: &'a Array<StateValue<InMemoryDB>, InMemoryDB>) -> Self {
        Self {
            array,
            _phantom: PhantomData,
        }
    }

    /// Returns the number of elements in the list.
    pub fn len(&self) -> usize {
        self.array.len()
    }

    /// Returns `true` if the list has no elements.
    pub fn is_empty(&self) -> bool {
        self.array.is_empty()
    }
}

impl<T> ListAccessor<'_, T>
where
    for<'vs> T: TryFrom<&'vs ValueSlice, Error = InvalidBuiltinDecode>,
{
    /// Returns the element at the given index, converting from on-chain representation.
    ///
    /// Returns `None` if the index is out of bounds. Returns `Some(Err(..))` if the
    /// element is present but cannot be converted.
    pub fn get(&self, index: usize) -> Option<Result<T, StateError>> {
        let sv = self.array.get(index)?;
        let av = match crate::nav::cell_value(sv) {
            Ok(av) => av,
            Err(e) => return Some(Err(e)),
        };
        Some(T::try_from(&*av.value).map_err(StateError::Conversion))
    }

    /// Iterates over all elements, converting each from on-chain representation.
    pub fn iter(&self) -> impl Iterator<Item = Result<T, StateError>> + '_ {
        self.array.iter().map(|sp| {
            let sv: &StateValue<InMemoryDB> = &sp;
            let av = crate::nav::cell_value(sv)?;
            T::try_from(&*av.value).map_err(StateError::Conversion)
        })
    }
}

/// Structural accessor for a Midnight contract merkle tree field.
///
/// Merkle trees only store leaf hashes, not the original typed values. This
/// accessor provides structural access to the tree: root hash, height, and the
/// next free slot index.
///
/// The on-chain layout is a compound `StateValue::Array` with 3 elements:
/// - `[0]`: `StateValue::BoundedMerkleTree(MerkleTree<(), InMemoryDB>)` — the live tree
/// - `[1]`: `StateValue::Cell(u64)` — `first_free` index counter
/// - `[2]`: `StateValue::Map(HashMap)` — history set (root hashes to Null)
pub struct MerkleTreeAccessor<'a> {
    tree: &'a MerkleTree<(), InMemoryDB>,
    first_free: u64,
}

impl std::fmt::Debug for MerkleTreeAccessor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MerkleTreeAccessor")
            .field("height", &self.tree.height())
            .field("first_free", &self.first_free)
            .finish()
    }
}

impl<'a> MerkleTreeAccessor<'a> {
    /// Creates a new accessor from the compound `StateValue::Array`.
    ///
    /// The on-chain layout is a 3-element array:
    /// - `[0]`: `StateValue::BoundedMerkleTree` — the merkle tree
    /// - `[1]`: `StateValue::Cell(u64)` — the `first_free` counter
    /// - `[2]`: `StateValue::Map` — history set (root hashes, used by `HistoricMerkleTree`)
    ///
    /// Elements `[0]` and `[1]` are required. Element `[2]` is present but not
    /// exposed through this accessor — use `from_state` on the parent `StateValue`
    /// to access the history map directly if needed.
    pub fn from_state(sv: &'a StateValue<InMemoryDB>) -> Result<Self, StateError> {
        let arr = match sv {
            StateValue::Array(arr) => arr,
            other => {
                return Err(StateError::UnexpectedVariant {
                    expected: "Array",
                    actual: crate::nav::variant_name(other),
                });
            }
        };

        // Element [0]: BoundedMerkleTree
        let tree_sv = arr.get(0).ok_or(StateError::IndexOutOfBounds(0))?;
        let tree = match tree_sv {
            StateValue::BoundedMerkleTree(t) => t,
            other => {
                return Err(StateError::UnexpectedVariant {
                    expected: "BoundedMerkleTree",
                    actual: crate::nav::variant_name(other),
                });
            }
        };

        // Element [1]: Cell containing u64 (first_free counter)
        let first_free = {
            let sv = arr.get(1).ok_or(StateError::IndexOutOfBounds(1))?;
            let av = crate::nav::cell_value(sv)?;
            u64::try_from(&*av.value).map_err(StateError::Conversion)?
        };

        Ok(Self { tree, first_free })
    }

    /// Returns the height of the merkle tree.
    pub fn height(&self) -> u8 {
        self.tree.height()
    }

    /// Returns the index of the next free slot.
    pub fn first_free(&self) -> u64 {
        self.first_free
    }

    /// Returns the root hash of the merkle tree, if the tree has been rehashed.
    pub fn root(&self) -> Option<MerkleTreeDigest> {
        self.tree.root()
    }
}
