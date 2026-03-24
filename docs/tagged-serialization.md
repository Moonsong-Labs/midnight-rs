# How `tagged_serialize` / `tagged_deserialize` works in midnight-ledger

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

- `StateValue` → `"impact-state-value[v2]"`
- `ContractState` → `"contract-state[v2]"`
- `u64` → `"u64"`
- `Option<Foo>` → `"option(foo)"`

The tag is declared via the `#[tag = "..."]` attribute on the `#[derive(Storable)]` macro.

## tagged_serialize

```rust
pub fn tagged_serialize<T: Serializable + Tagged>(value: &T, writer: impl Write) -> io::Result<()> {
    let tag = T::tag();
    write!(writer, "midnight:{tag}:")?;  // write the tag prefix
    value.serialize(writer)               // write the serialized data
}
```

It writes:
1. The string `"midnight:"` (global prefix, defined as `GLOBAL_TAG`)
2. The type's tag (e.g., `"impact-state-value[v2]"`)
3. A `":"` separator
4. The binary serialized data from `T::serialize()`

For `StateValue`, the output bytes look like:

```
midnight:impact-state-value[v2]:<binary data>
```

The tag is plain ASCII text. The binary data follows immediately after the last `":"`.

## tagged_deserialize

```rust
pub fn tagged_deserialize<T: Deserializable + Tagged>(reader: impl Read) -> io::Result<T> {
    // 1. Compute expected tag
    let expected = format!("midnight:{}:", T::tag());

    // 2. Read that many bytes from the stream
    let mut read_tag = vec![0u8; expected.len()];
    reader.read_exact(&mut read_tag)?;

    // 3. Compare — reject if mismatch
    if read_tag != expected.as_bytes() {
        return Err(io::Error::new(InvalidData, "expected header tag '...'"));
    }

    // 4. Deserialize the value from the remaining bytes
    let value = T::deserialize(reader, 0)?;

    // 5. Verify no trailing bytes remain
    if reader.bytes().count() != 0 {
        return Err(io::Error::new(InvalidData, "trailing bytes"));
    }

    Ok(value)
}
```

Steps:
1. Build the expected tag string: `"midnight:<type tag>:"`
2. Read exactly that many bytes from the input
3. If they don't match, return an error (wrong type or corrupt data)
4. Call `T::deserialize()` to reconstruct the value from the remaining bytes
5. Verify the stream is fully consumed (no extra bytes)

The tag acts as a type check — you can't accidentally deserialize a `ContractState` from bytes that were serialized as a `StateValue`.

## How StateValue serialization works

`StateValue` is an enum with variants: `Null`, `Cell(Sp<AlignedValue>)`, `Map(HashMap<...>)`, `Array(Array<...>)`, `BoundedMerkleTree(MerkleTree<...>)`.

It derives `Storable`, which generates both `Storable` (per-node) and `Serializable` (full value) impls.

### Storable (per-node, used in ParityDB)

`to_binary_repr` writes only the node's own data:
- `Null` → discriminant byte `0x00`
- `Cell` → discriminant `0x01` (the `AlignedValue` is a child `Sp`, NOT included)
- `Map` → discriminant `0x02` (the `HashMap` is a child, NOT included)
- etc.

Children are stored separately in ParityDB as independent nodes referenced by `ArenaHash`.

### Serializable (full value, used in RPC)

When `StateValue` implements `Serializable`, it delegates to the `Sp` wrapper:

```rust
impl<T: Storable<D>, D: DB> Serializable for Sp<T, D> {
    fn serialize(&self, writer: &mut impl Write) -> io::Result<()> {
        self.serialize_to_node_list().serialize(writer)
    }
}
```

`serialize_to_node_list()` walks the entire sub-DAG and produces a `TopoSortedNodes`:

1. Starting from the root node, BFS to collect all reachable nodes
2. Topologically sort them (Kahn's algorithm) — leaves first, root last
3. Each node becomes a `TopoSortedNode { child_indices: Vec<u64>, data: Vec<u8> }`
4. Child references are stored as indices into the node list (not hashes)

The `TopoSortedNodes` is then serialized as:
- `u32`: number of nodes
- For each node:
  - `u32`: number of child indices
  - `u64[]`: child indices
  - `u32`: data length
  - `u8[]`: data bytes

## What the client receives

When the node returns a `tagged_serialize`'d `StateValue`, the bytes contain:

```
"midnight:impact-state-value[v2]:"     ← tag (36 bytes ASCII)
<u32: node_count>                       ← number of nodes in the TopoSortedNodes
  <node 0: child_indices + data>        ← leaf node (e.g., the AlignedValue)
  <node 1: child_indices + data>        ← root node (e.g., StateValue::Cell discriminant)
```

For a simple `Cell` containing an `AlignedValue`, there are 2 nodes:
- Node 0: the `AlignedValue` data (leaf, no children)
- Node 1: the `StateValue::Cell` discriminant with one child index pointing to node 0

The client calls `tagged_deserialize::<StateValue<InMemoryDB>>()` which:
1. Checks the tag matches `"midnight:impact-state-value[v2]:"`
2. Reads the `TopoSortedNodes`
3. Reconstructs the DAG: creates `Sp` wrappers for each node, linking children by index
4. Returns the root `StateValue` with lazy `Sp` pointers to children (in `InMemoryDB`)

## Why tagged_serialize for the query RPC

The `midnight_queryContractState` response uses `tagged_serialize` for map entry values because:

1. It produces a self-contained blob — children are included (unlike `to_binary_repr` which omits them)
2. The client can deserialize with `tagged_deserialize` — the same code used for `midnight_contractState`
3. The tag provides type safety — the client verifies it's receiving a `StateValue`
4. For a single map entry (e.g., `StateValue::Cell(AlignedValue)`), the blob is tiny — just 2 nodes

The alternative (`to_binary_repr`) would only give the discriminant byte without the actual value data. The `Sp<AlignedValue>` child would be missing.
