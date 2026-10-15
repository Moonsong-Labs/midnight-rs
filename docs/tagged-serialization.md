# Tagged serialization in midnight-ledger

> Internal reference for SDK developers. Users of midnight-rs don't need to understand this. `state::fetch_state()` and `state::fetch_state_from_node()` handle it automatically.

## Overview

midnight-ledger has two serialization layers:

1. **`Storable`** (DAG storage) — serializes a single node's data, excluding children. Used for persisting nodes in ParityDB.
2. **`Serializable`** (transport) — serializes a complete value including all reachable children. Used for RPC responses and data interchange.

`tagged_serialize` / `tagged_deserialize` are the top-level entry points for the transport layer. They add a type tag prefix for type safety.

## The tag

Every serializable type implements the `Tagged` trait:

```rust
pub trait Tagged {
    fn tag() -> Cow<'static, str>;
}
```

This returns a string that uniquely identifies the type and its version. Examples:

- `StateValue` → `"impact-state-value[v2]"` (version may differ across ledger releases)
- `ContractState` → `"contract-state[v6]"`
- `u64` → `"u64"`
- `Option<Foo>` → `"option(foo)"`

The tag is declared via the `#[tag = "..."]` attribute on the `#[derive(Storable)]` macro.

## How midnight-rs uses it

`state::fetch_state()` and `state::fetch_state_from_node()`:
1. Hex-decode the string from the provider.
2. Call `midnight_typed_state::decode_contract_state`, which reads the tag through `tagged_deserialize` and converts a ledger 8 state.
3. Return the `ContractState` for the interpreter.

## tagged_serialize

```rust
pub fn tagged_serialize<T: Serializable + Tagged>(value: &T, writer: impl Write) -> io::Result<()> {
    let tag = T::tag();
    write!(writer, "midnight:{tag}:")?;
    value.serialize(writer)
}
```

Output format: `midnight:<type_tag>:<binary data>`

## tagged_deserialize

```rust
pub fn tagged_deserialize<T: Deserializable + Tagged>(reader: impl Read) -> io::Result<T> {
    // 1. Read and verify the tag matches the expected type
    // 2. Deserialize the value from remaining bytes
    // 3. Verify no trailing bytes remain
}
```

The tag acts as a type check — you can't accidentally deserialize a `ContractState` from bytes that were serialized as a `StateValue`.

## StateValue serialization

`StateValue` is an enum: `Null`, `Cell(AlignedValue)`, `Map(HashMap)`, `Array(Array)`, `BoundedMerkleTree(MerkleTree)`.

The `Serializable` impl walks the entire sub-DAG and produces a `TopoSortedNodes`:

1. BFS to collect all reachable nodes
2. Topological sort (leaves first, root last)
3. Each node becomes `{ child_indices: Vec<u64>, data: Vec<u8> }`
4. Child references are indices into the node list (not hashes)

Binary format:
```
<u32: node_count>
  per node:
    <u32: child_count> <u64[]: child_indices>
    <u32: data_length> <u8[]: data>
```

## How the node RPC returns state

The `midnight_queryContractState` response uses `tagged_serialize` for individual field values. The `midnight_contractState` response is a hex-encoded `tagged_serialize`'d full `ContractState`.

`state::fetch_state()` and `state::fetch_state_from_node()` decode the `midnight_contractState` hex string.
