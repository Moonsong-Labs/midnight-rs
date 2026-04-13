# midnight-rs — Architecture

Rust SDK for the Midnight blockchain. Covers the full contract lifecycle: deploy, query state, execute circuits, build transactions, generate ZK proofs, and submit to the network.

## Architecture

```
midnight-core (meta-crate, re-exports all public API)
  ├── midnight-contract
  │     ├── interpreter   — circuit IR execution engine + WitnessProvider trait
  │     ├── call          — transaction builder (deploy, prove, submit)
  │     ├── contract      — Contract<P> (stateless handle) + typestate DeployBuilder / ConnectBuilder (IntoFuture)
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

All crates live in a single workspace rooted at `crates/`.

## Data flow

### Query state

```
Provider.get_contract_state(address)
  → indexer GraphQL → hex string
  → Ledger::from_hex(hex) → tagged_deserialize → ContractState<InMemoryDB>
  → Generated Ledger struct with typed field accessors
```

### Call a circuit (on-chain, via Contract)

```
contract.circuits(&witnesses).increment_by(5).await
  ↓
fetch_state(provider, address, at_block)   // fresh state per call
  ↓
interpreter::execute_with(ir, state, args, witnesses, ...)
  → ExecutionResult { state, reads, gather_ops, communication_outputs, result }
  ↓
gather_ops → translate → verify_ops → partition_transcripts
  → serialize across InMemoryDB→DefaultDB boundary
  ↓
sync wallet → load Resolver → build CallAction
  (output = communication commitment from disclose() calls)
  → StandardTransactionInfo → proven tx_bytes
  ↓
submit(node_url, tx_bytes) → wait for indexer confirmation
  ↓
decode typed return value from result and hand it back to the caller
```

### Deploy

```
Contract::deploy(&provider)                    // DeployBuilder<'_, P>
  .with_initial_state(LedgerInitialState::default())
  .with_zk_keys("compiled")
  .await                                       // IntoFuture
  ↓
with_zk_keys(state, keys_dir)  → load .verifier files into state.operations
  ↓
deploy_funded(state, node_url, wallet_seed)
  → sync wallet → build funded deploy TX → prove → submit
  ↓
wait_for_deployment(provider, address, timeout, poll_interval)
  → poll indexer until contract exists
  ↓
return Contract<P> (stateless handle, no cached state)
```

### Connect

```
Contract::connect(&provider, address)          // ConnectBuilder<'_, P>
  .with_zk_keys("compiled")
  .await                                       // IntoFuture
  ↓
return Contract<P> (stateless handle, no state fetched)
```

`Contract<P>` is a stateless, immutable handle. It does not cache contract
state. Ledger queries go through `contract.ledger().await?`, which fetches
the full contract state via the `midnight_contractState` node RPC and
returns a sync `Ledger` struct with typed field accessors. For per-field
lazy queries (custom node builds only), use `contract.ledger_query()`.
Circuit calls via `contract.circuits(&w)` fetch fresh state before execution.

## Documentation Index

| Document | What it covers |
|----------|---------------|
| `aligned-value-navigation.md` | `AlignedValue` internals and state tree structure |
| `compact-adt-state-mapping.md` | Compact storage kinds → `StateValue` mapping |
| `tagged-serialization.md` | midnight-ledger's tagged serialization format |

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
| State change subscriptions | WebSocket subscription support |
| Connection auto-retry | `MidnightProvider` clears stale connections on failure but does not automatically retry |
| Lazy query batching | Each accessor makes a separate RPC call |
| Production proving | Uses test-utilities; not mainnet-ready |
