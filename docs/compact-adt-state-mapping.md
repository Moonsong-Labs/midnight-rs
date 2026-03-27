# Compact ADT to StateValue mapping

> How the Compact compiler's high-level ledger ADTs map to the low-level `StateValue` enum in midnight-ledger's Rust runtime.

## StateValue variants

The on-chain state tree is built from five primitives defined in [`midnight-ledger/onchain-state/src/state.rs`](https://github.com/midnightntwrk/midnight-ledger/blob/d245ff7069f3dc74f5d3e7f23b26fbc0acd152d7/onchain-state/src/state.rs#L79):

```rust
pub enum StateValue<D: DB = InMemoryDB> {
    Null,                                        // tag 0
    Cell(Sp<AlignedValue, D>),                   // tag 1
    Map(HashMap<AlignedValue, StateValue<D>, D>), // tag 2
    Array(Array<StateValue<D>, D>),              // tag 3
    BoundedMerkleTree(MerkleTree<(), D>),         // tag 4
}
```

## Compact ADTs

The Compact compiler defines seven ledger ADTs in `compiler/midnight-ledger.ss` using `declare-ledger-adt`. Each ADT specifies an `initial-value` using `state-value` tags that map directly to `StateValue` variants.

### Summary

| Compact ADT | `StateValue` encoding | Initial value |
|---|---|---|
| `Cell<T>` | `Cell(value)` | `Cell(rt-null T)` |
| `Counter` | `Cell(u64)` | `Cell(align 0 8)` — a zero u64 |
| `Set<T>` | `Map({})` | `Map({})` — empty map |
| `Map<K, V>` | `Map({})` | `Map({})` — empty map |
| `List<T>` | `Array([head, tail, len])` | `Array([Null, Null, Cell(0)])` |
| `MerkleTree<n, T>` | `Array([BoundedMerkleTree, free_idx])` | `Array([BoundedMerkleTree(n, []), Cell(0)])` |
| `HistoricMerkleTree<n, T>` | `Array([BoundedMerkleTree, ...])` | Similar to MerkleTree with additional history state |

### Cell\<T\>

A single value wrapper. This is the only ADT users never write explicitly — when a ledger field is declared as `ledger x: Field`, the compiler implicitly wraps it in `Cell<Field>`. In the compiler IR the ADT is renamed to `__compact_Cell` to distinguish it from user-written type references.

```
StateValue::Cell(aligned_value)
```

Operations: `read`, `write`, `resetToDefault`.

### Counter

A u64 counter stored as a single `Cell` containing an 8-byte aligned value.

```
StateValue::Cell(align(0, 8))   // 8 bytes, initialized to 0
```

Operations: `read`, `lessThan`, `increment(Uint16)`, `decrement(Uint16)`, `resetToDefault`.

Note: increment/decrement use VM `addi`/`subi` instructions directly on the cell value, so Counter is not a `Map` or `Array` — it's a plain `Cell` with arithmetic operations.

### Set\<T\>

An unbounded set, implemented as a `Map` where keys are the set elements. The values in the map are not meaningful — membership is determined by key presence.

```
StateValue::Map({
    aligned_elem_1 => ...,
    aligned_elem_2 => ...,
})
```

Operations: `member`, `insert`, `remove`, `isEmpty`, `size`, `resetToDefault`.

### Map\<K, V\>

An unbounded key-value map. Each entry maps an `AlignedValue` key to a `Cell`-wrapped value.

```
StateValue::Map({
    aligned_key => StateValue::Cell(aligned_value),
    ...
})
```

Operations: `has`, `lookup`, `insert`, `update`, `remove`, `isEmpty`, `size`, `resetToDefault`.

The value type `V` can itself be an ADT (the `ADT/Type` parameter kind), allowing nested structures like `Map<Field, Set<Field>>`.

### List\<T\>

A linked list encoded as a 3-element `Array` representing `[head, tail, length]`:

```
StateValue::Array([
    StateValue::Cell(head_value) | StateValue::Null,  // head
    StateValue::Array([...]) | StateValue::Null,       // tail (recursive)
    StateValue::Cell(align(length, 8)),                // length as u64
])
```

An empty list is `Array([Null, Null, Cell(0)])`. Each `push` prepends a new head node.

Operations: `head`, `push`, `pop`, `nth`, `isEmpty`, `size`, `resetToDefault`.

### MerkleTree\<n, T\>

A bounded Merkle tree of depth `n` (where `2 ≤ n ≤ 32`), stored as a 2-element `Array`:

```
StateValue::Array([
    StateValue::BoundedMerkleTree(height=n, leaves=[...]),
    StateValue::Cell(align(first_free, 8)),  // next free leaf index
])
```

Operations: `insert`, `isEmpty`, `size`, `resetToDefault` (plus JS-only: `root`, `first_free`, `path_for_leaf`).

### HistoricMerkleTree\<n, T\>

Similar to `MerkleTree` but with additional state for history tracking (previous roots/snapshots). Uses the same `Array` + `BoundedMerkleTree` encoding with extra entries.

## Where the definitions live

| Layer | Location | Role |
|---|---|---|
| ADT declarations | `compiler/midnight-ledger.ss` | Defines type params, initial values, and VM operations |
| Macro expansion | `compiler/ledger.ss` | `declare-ledger-adt` macro, collects definitions |
| Compiler integration | `compiler/analysis-passes.ss` | Injects ADTs into the standard library environment |
| On-chain runtime | `midnight-ledger/onchain-state/src/state.rs` | `StateValue` enum that the VM operates on |

The Compact compiler and the Rust runtime define these structures independently — they must stay in sync by convention, not by code generation.
