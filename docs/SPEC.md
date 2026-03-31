# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle: deploy, query state, execute circuits, build transactions, generate ZK proofs, and submit to the network.

## Architecture

```
midnight-core (meta-crate, re-exports all public API)
  ├── midnight-contract
  │     ├── interpreter   — circuit IR execution engine
  │     ├── call          — transaction builder (deploy, prove, submit)
  │     ├── contract      — Contract<P> typed wrapper + ContractBuilder
  │     ├── midnight-provider
  │     │     ├── midnight-indexer-client (GraphQL)
  │     │     └── subxt / jsonrpsee (node RPC)
  │     ├── midnight-bindgen (codegen from contract-info.json)
  │     │     ├── midnight-bindgen-macro → compact-codegen
  │     │     └── midnight-bindgen-runtime → midnight-ledger crates
  │     ├── compact-codegen (IR types + code generation)
  │     ├── midnight-ledger (transaction types, proving, crypto)
  │     └── midnight-node-ledger-helpers (DustWallet, wallet infrastructure)
  └── (re-exports)
```

All crates live in a single workspace. See `docs/crate-map.md` for the full layout.

## Data flow

### Query state

```
Provider.get_contract_state(address)
  → indexer GraphQL → hex string
  → Ledger::from_hex(hex) → tagged_deserialize → ContractState<InMemoryDB>
  → Generated Ledger struct with typed field accessors
```

### Call a circuit (local, generated method)

```
ledger.call_increment()
  → deserializes embedded IR JSON constant
  → interpreter::execute_with(ir, state, args, NoWitnesses, helpers)
  → returns new Ledger wrapping updated state
```

### Call a circuit (on-chain, via Contract)

```
contract.circuits().increment().await
  ↓
interpreter::execute_with(ir, state, ...)
  → ExecutionResult { state, reads, gather_ops }
  ↓
gather_ops → translate → verify_ops → partition_transcripts
  → serialize across InMemoryDB→DefaultDB boundary
  ↓
sync wallet → load Resolver → build CallAction
  → StandardTrasactionInfo → proven tx_bytes
  ↓
submit(node_url, tx_bytes) → wait for indexer confirmation
```

### Deploy

```
Contract::deploy()
  .provider(&provider)
  .initial_state(LedgerInitialState { ... })
  .zk_keys("compiled")
  .deploy().await
  ↓
with_zk_keys(state, keys_dir)  → load .verifier files into state.operations
  ↓
deploy_funded(state, node_url, wallet_seed)
  → sync wallet → build funded deploy TX → prove → submit
  ↓
wait_for_deployment(provider, address, timeout, poll_interval)
  → poll indexer until state appears
```

## Documentation Index

| Document | What it covers |
|----------|---------------|
| `crate-map.md` | Workspace layout, dependency graph, "which crate to modify" guide |
| `circuit-execution-architecture.md` | Three representations (VM Ops, Circuit IR, ZKIR) and how they relate |
| `codegen-guide.md` | Code generation pipeline, what gets generated, type mappings |
| `transaction-pipeline.md` | How circuit calls become on-chain transactions |
| `interpreter-reference.md` | IR interpreter: nodes, builtins, ledger query execution |
| `testing-guide.md` | Test locations, coverage, gaps, how to add tests |
| `known-issues.md` | Limitations, gotchas, and technical debt |
| `aligned-value-navigation.md` | AlignedValue internals and state tree structure |
| `compact-adt-state-mapping.md` | Compact storage kinds → StateValue mapping |
| `tagged-serialization.md` | midnight-ledger's serialization format |

## External dependencies

| Crate | Source | Purpose |
|---|---|---|
| midnight-ledger | midnightntwrk/midnight-ledger | Transaction types, proving, crypto, VM |
| midnight-node-ledger-helpers | RomarQ/midnight-node | DustWallet, wallet infrastructure |
| midnight-node-toolkit | RomarQ/midnight-node | Block fetching, context sync |
| pallet-midnight-rpc | RomarQ/midnight-node | query_contract_state RPC types |
| subxt | crates.io | Substrate RPC, extrinsic submission |
| jsonrpsee | crates.io | WebSocket RPC client |

## Not yet implemented

| Feature | Notes |
|---|---|
| Typed circuit arguments for on-chain calls | `Circuits` struct skips circuits with args |
| State change subscriptions | WebSocket subscription support |
| Connection auto-retry | MidnightProvider clears stale connections on failure but does not automatically retry |
| Lazy query batching | Each accessor makes a separate RPC call |
| Production proving | Uses test-utilities; not mainnet-ready |
