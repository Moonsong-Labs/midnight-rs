# Dust and Fee Payment on Midnight

## Overview

Every Midnight transaction costs a fee denominated in DUST (atomic unit: SPECK).
There is only one practical way to pay fees for normal transactions: spending
real dust UTXOs. A separate one-time bootstrap mechanism ("generationless fee
availability") exists solely for the initial dust registration transaction.

## tNIGHT, DUST, and Units

| Token  | Atomic unit | Ratio              |
|--------|-------------|--------------------|
| tNIGHT | STAR        | 1 tNIGHT = 10^6 STAR |
| DUST   | SPECK       | 1 DUST = 10^15 SPECK  |

The ledger parameters define the relationship between NIGHT holdings and DUST:

- `night_dust_ratio` (currently 5,000,000,000): max DUST capacity per NIGHT in
  SPECK/STAR. Equivalently, 5 DUST per 1 tNIGHT.
- `generation_decay_rate` (currently 8,267): generation rate in SPECK/STAR/second.
- Time to full capacity: `night_dust_ratio / generation_decay_rate` ~ 604,814
  seconds (~7 days).

Dust generation is purely time-based. Holding tNIGHT is sufficient; no staking
or external chain interaction is required.

## How Fees Work: The Two Mechanisms

### 1. Generationless Fee Availability (one-time bootstrap only)

A wallet holding tNIGHT UTXOs accrues "virtual dust" over time. This virtual
dust exists only for the purpose of paying the fee on a **dust registration
transaction**. It cannot be used for normal transfers, contract calls, or any
other transaction type.

#### The formula

Computed per tNIGHT UTXO:

```
dt = current_time - utxo_creation_time
rate = utxo_value * generation_decay_rate
virtual_dust = min(dt * rate, utxo_value * night_dust_ratio)
```

Total virtual dust is the sum across all tNIGHT UTXOs.

#### Why it only works once

Two protocol constraints make this a one-time mechanism:

1. **Requires `dust_actions` in the intent.** The ledger's
   `generationless_fee_availability()` function reads
   `parent_intent.dust_actions.ctime` to compute the UTXO age. Only
   transactions that include a `DustRegistrationBuilder` produce
   `dust_actions`. Normal transactions (transfers, contract calls) use
   `set_funding_seeds()` instead, which goes through the `gather_dust_spends()`
   path and requires actual dust UTXOs.

2. **`night_indices` filter.** After the first dust registration, each NIGHT
   UTXO's nonce is stored in the ledger's `night_indices` set via
   `fresh_dust_output()`. On any subsequent call, `generationless_fee_availability()`
   skips UTXOs whose nonces are already in `night_indices`, returning 0
   virtual dust. The only way to get generationless availability again is to
   spend and re-create the tNIGHT UTXOs (producing new nonces), but this itself
   requires a fee, creating a circular dependency.

#### How to use it

Include a `DustRegistrationBuilder` in the transaction. The transaction must
also spend and re-create the tNIGHT UTXOs in an unshielded offer (the node
validates the inputs exist and computes their age):

```rust
tx_info.add_dust_registration(DustRegistrationBuilder {
    signing_key,                        // from UnshieldedWallet
    dust_address: Some(dust_public_key), // from DustWallet
    allow_fee_payment,                  // computed virtual dust amount
});
```

### 2. Spending Dust UTXOs (normal fee payment)

This is the mechanism used by all wallets (including Lace) for every normal
transaction after the initial dust registration.

After dust registration, each tNIGHT UTXO "generates" a corresponding dust UTXO
on-chain. These are real UTXOs tracked in two global Merkle trees (commitment
tree and generation tree). The dust grows over time following the same linear
formula as generationless availability, but tracked on-chain as spendable state.

Normal transactions pay fees by calling `set_funding_seeds()` on the
`StandardTrasactionInfo`. Internally, `pay_fees()` calls `gather_dust_spends()`,
which calls `DustWallet::speculative_spend()` to produce ZK proofs with valid
Merkle paths from both trees.

#### What this requires

The wallet must replay **all** `dustLedgerEvents` from genesis, sequentially.
Both Merkle trees are global (they contain entries for every wallet on the
network), and the replay enforces strict sequential insertion
(`mt_index == expected_next`). Skipping events or starting from a non-zero
index is not supported.

On preprod, this takes ~30 minutes from genesis (~500k+ events). Subsequent
syncs resume from the last saved cursor and are fast.

Reverse-order replay (newest to oldest) is not possible because Merkle tree
construction depends on prior insertions. The trees enforce sequential insertion
via `mt_index == expected_next` checks.

#### Why a fresh wallet still replays from genesis

The dust Merkle trees are **global**, not per-wallet. A fresh wallet's dust
UTXOs are leaves in a tree that contains every wallet's UTXOs on the entire
network. To spend a dust UTXO at position #523,847, `DustLocalState::spend()`
calls `commitment_tree.path_for_leaf()` and `generating_tree.path_for_leaf()`,
which compute sibling hashes from the target leaf up to the root. That path
depends on every other leaf in the tree, including those inserted before the
wallet existed.

**This is an implementation limitation, not a protocol requirement.** The ZK
proof only needs the Merkle *path* (log2(N) sibling hashes), not the full
tree. See "Possible optimizations" below for approaches that could eliminate
the genesis replay.

#### Operational impact

Developers building on Midnight hit this in practice. Common issues reported
on the forum and in community channels:

- ~15 minute cold starts for dust sync on preprod (longer as the chain grows).
- `InsufficientFunds` errors when Merkle roots go stale. The dust root history
  has a pruning window of ~1 hour. If the wallet sits idle between sync and
  submit, roots get pruned and spend proofs become invalid, even though
  `dust.balance(now)` still reports a large value.
- The recommended workaround is a long-running daemon that keeps dust state
  warm with continuous polling (~3s intervals), resyncing a delta before
  each transaction submission.

#### Possible optimizations

The following approaches could eliminate the genesis replay requirement.
None exist in the current codebase. No open issues or proposals for them
were found in the midnight-indexer or midnight-node repositories.

1. **Indexer-provided Merkle paths on demand**: the indexer could serve
   current paths for specific UTXOs (similar to Bitcoin SPV). The wallet
   would need ~2.2 KB per UTXO instead of the full tree. The indexer
   already computes and stores Merkle paths internally
   (`DustGenerationDtimeUpdate` events contain `merkle_path: Vec<DustMerklePathEntry>`
   in the domain model and in the database JSON column), but the GraphQL
   API layer strips these fields before serving them to clients.

2. **Re-expose stripped fields**: the simplest change. The indexer's
   `DustGenerationDtimeUpdate` and `DustInitialUtxo` GraphQL types could
   include the `generation_info`, `generation_index`, and `merkle_path`
   fields that are already stored. This is a ~150-line change in the API
   layer with no backend or schema changes. However, paths from historical
   events become stale as the tree grows, so this alone doesn't eliminate
   the need to process all events. It does eliminate the need to maintain
   the full tree in memory.

3. **Tree snapshots**: both trees implement `Serialize`/`Deserialize`. A
   trusted snapshot at a recent block would let fresh wallets start from
   there instead of genesis. Adds a large payload (~1-100 MB depending
   on tree fullness).

4. **Node-side proof generation**: the node already has the full trees. It
   could generate dust spend proofs server-side, eliminating client-side
   tree maintenance entirely.

#### Why this hasn't been done (probable reason)

No official rationale has been stated. The dust spec
(`midnight-ledger/spec/dust.md`) describes a privacy technique for wallet
recovery: wallets should query commitments using bit-prefixes ("stochastic
filtering") rather than exact lookups, so the indexer cannot learn which
specific UTXOs a wallet owns. Serving exact Merkle paths for specific UTXOs
on demand would reveal which UTXOs the wallet intends to spend, undermining
this privacy property. This is consistent with the overall architecture
where clients build proofs locally and the indexer only serves opaque event
streams.

## Wallet Lifecycle

```
1. Fund wallet with tNIGHT
   |
2. Unshielded sync (~seconds) -- discovers tNIGHT UTXOs
   |
3. Dust registration transaction (uses generationless fee availability)
   |  -- this is the ONLY transaction that can use virtual dust for fees
   |  -- links NIGHT address to DUST address
   |  -- creates initial dust UTXOs on-chain
   |
4. Full dust sync (~30 min from genesis) -- replays dustLedgerEvents
   |  -- builds commitment tree + generation tree (global Merkle trees)
   |  -- required before any further transactions
   |
5. Normal transactions (transfers, contract calls, etc.)
      -- all pay fees by spending real dust UTXOs via set_funding_seeds()
```

## Dust Registration

Dust registration links a NIGHT address to a DUST address. It serves two
purposes in a single transaction:

1. **Address delegation**: tells the network which dust public key receives dust
   for this NIGHT address.
2. **Fee payment**: the `allow_fee_payment` field lets the transaction consume
   virtual dust (generationless availability) to pay its own fees.

After registration, the on-chain `apply_registration` function:

- Records the address delegation.
- Computes `dust_in` from generationless availability of the NIGHT inputs.
- Pays `fee_paid = min(fees_remaining, min(allow_fee_payment, dust_in))`.
- Creates dust UTXOs for each NIGHT output with `initial_value` proportional
  to the remaining virtual dust (`dust_in - fee_paid`).
- Stores NIGHT UTXO nonces in `night_indices` (preventing future generationless
  availability for these UTXOs).

## Wallet Sync Phases

The wallet sync has three independent phases:

1. **Unshielded sync** (~seconds): discovers tNIGHT UTXOs via the
   `unshieldedTransactions` subscription. Required for balance display and
   all transaction types.

2. **Zswap sync** (~seconds): replays `zswapLedgerEvents` for shielded coin
   tracking (Merkle tree). Required for shielded balance and shielded
   transfers.

3. **Dust sync** (~30 min from genesis): replays `dustLedgerEvents` for dust
   UTXO tracking (two global Merkle trees). Required for all transactions
   after the initial dust registration.

All three phases run concurrently inside `MidnightProvider::sync_wallet()`
(and `MidnightProvider::sync_wallet_with_progress()` for streamed progress
updates), which drives the wallet sync against the provider's indexer.

A brand-new wallet can submit exactly one transaction (dust registration)
without a dust sync. After that, the dust sync is mandatory.

## What Lace / the Midnight Toolkit Does

The official Midnight toolkit (used by Lace wallet) follows this exact pattern:

- `register_dust_address.rs`: uses `add_dust_registration()` with generationless
  fees for the one-time registration.
- `contract_call.rs`, `single_tx.rs`, and all other transaction builders: use
  `set_funding_seeds()`, which requires real dust UTXOs with valid Merkle proofs.

There is no general-purpose "virtual dust" payment path in the toolkit. Every
normal transaction requires a fully synced `DustWallet` with up-to-date Merkle
trees.
