# AlignedValue and State Navigation

## What is AlignedValue?

`AlignedValue` is the universal value type in Midnight's Compact runtime. Every piece of data stored in contract state, passed to circuits, or used as a map key is represented as an `AlignedValue`.

```rust
pub struct AlignedValue {
    pub value: Value,           // the data
    pub alignment: Alignment,   // describes the data's type/shape
}
```

It consists of two parts:

### Value

A `Value` is a sequence of `ValueAtom`s. Each `ValueAtom` is a byte vector.

```rust
pub struct Value(pub Vec<ValueAtom>);
pub struct ValueAtom(pub Vec<u8>);
```

Simple types produce a single atom:
- `u64(42)` → one atom: `[0x2a]` (42 as bytes, trailing zeros stripped)
- `bool(true)` → one atom: `[0x01]`
- `Bytes<32>` → one atom: 32 bytes

Compound types (structs, tuples) produce multiple atoms — one per field:
- `{ a: u64, b: u64 }` → two atoms: `[bytes_of_a, bytes_of_b]`

### Alignment

An `Alignment` describes the type structure — how many atoms there are and what each represents.

```rust
pub struct Alignment(pub Vec<AlignmentSegment>);

pub enum AlignmentAtom {
    Bytes { length: u32 },   // fixed-size byte string (u8=1, u16=2, u32=4, u64=8, u128=16)
    Field,                    // a Jubjub scalar field element (Fr, ~32 bytes)
}
```

Examples:
- `u64` alignment: `Bytes { length: 8 }` — serialization tag `0x08`
- `Fr` (Compact `Field`) alignment: `Field` — serialization tag `0x41`
- `{ a: u64, b: u64 }` alignment: `[Bytes(8), Bytes(8)]` — tag `0x08, 0x08`
- `Bytes<32>` alignment: `Bytes { length: 32 }` — serialization tag `0x20`

The alignment byte is why the same numeric value serializes differently depending on type:

| Type | Value | Serialized hex |
|------|-------|----------------|
| `u64(42)` | 42 | `412a08` |
| `Fr(42)` | 42 | `412a41` |
| `u8(0)` | 0 | `4001` |
| `Fr(0)` | 0 | `4041` |

The last byte (`08`, `41`, `01`) is the alignment — it tells the deserializer how to interpret the preceding data.

## Types that convert to AlignedValue

Any type implementing the `Aligned` trait can be converted to `AlignedValue` via `Into`:

| Rust type | Alignment | Maps to Compact type |
|-----------|-----------|---------------------|
| `u8` | `Bytes(1)` | `Uint<8>` |
| `u16` | `Bytes(2)` | `Uint<16>` |
| `u32` | `Bytes(4)` | `Uint<32>` |
| `u64` | `Bytes(8)` | `Uint<64>` / `Counter` |
| `u128` | `Bytes(16)` | `Uint<128>` |
| `bool` | `Bytes(1)` | `Boolean` |
| `[u8; N]` | `Bytes(N)` | `Bytes<N>` |
| `Fr` | `Field` | `Field` |
| tuples | concat alignments | structs |

## AlignedValue as a universal key

In the Compact VM, the `idx` instruction navigates into any `StateValue` node using an `AlignedValue` as the key. The interpretation depends on the node type:

| StateValue variant | Key interpretation | Example |
|---|---|---|
| `Array` | Convert to `u8` index | `u8(0).into()` → Array element 0 |
| `Map` | Use directly as HashMap key | `Fr(42).into()` → Map entry for key 42 |
| `BoundedMerkleTree` | Convert to `u64` position | `u64(5).into()` → Leaf at position 5 |

This is why `AlignedValue` works as the universal path step in `midnight_queryContractState`: each step is an `AlignedValue` serialized as bytes, and the node type determines how it's used.

## How navigation works

The state tree of a Compact contract is nested `StateValue` nodes:

```
StateValue::Array                        ← top level (ledger fields)
  ├── [0] StateValue::Map(HashMap)       ← egress_jobs: Map<Field, EgressJob>
  │         ├── Fr(1) → Cell(EgressJob)
  │         ├── Fr(2) → Cell(EgressJob)
  │         └── Fr(3) → Cell(EgressJob)
  └── [1] StateValue::Cell(u64)          ← job_count: Counter
```

To read `egress_jobs[Fr(1)]`:

1. Start at root (Array)
2. Step with `u8(0)` → Array interprets as index 0 → reaches the Map
3. Step with `Fr(1)` → Map interprets as key → reaches Cell(EgressJob)
4. Serialize the Cell → return to client

The RPC query for this:

```json
{
  "method": "midnight_queryContractState",
  "params": [
    "<contract_address>",
    [{ "path": ["4001", "0141"] }],
    null
  ]
}
```

Where `"4001"` is serialized `u8(0)` and `"0141"` is serialized `Fr(1)`.

## Serialization format

`AlignedValue` serializes (via `Serializable`) as:

```
<ValueSlice serialization><Alignment serialization>
```

Where `ValueSlice` serializes each atom with a flagged-integer length prefix:

- Single atom with value < 32: one byte (the value itself with flags)
- Single atom with value ≥ 32: flagged length + raw bytes
- Multiple atoms: encoded as a sequence

The alignment follows as a compact representation of the type structure.

This is the format used in the `path` elements of `midnight_queryContractState` queries. The client serializes each key using the `Serializable` trait (or the bindgen's generated `Into<AlignedValue>` conversions), hex-encodes it, and sends it as a path step.
