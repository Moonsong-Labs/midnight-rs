# Intents, ZswapOffers, and Contract Effects

How transactions, intents, ZswapOffers, and contract effects fit together in the Midnight ledger. Based on the [intents-transactions spec](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/intents-transactions.md) (the `Transaction` and `Intent` struct shapes below are verified against `main` as of this writing).

## Transaction structure

A transaction is composed of:

```
Transaction {
    intents:          Map<u16, Intent>,        // keyed by segment_id (never 0)
    guaranteed_offer: Option<ZswapOffer>,       // segment 0
    fallible_offer:   Map<u16, ZswapOffer>,     // keyed by segment_id (never 0)
    binding_randomness: Fr,
}
```

An intent is:

```
Intent {
    guaranteed_unshielded_offer: Option<UnshieldedOffer>,
    fallible_unshielded_offer:   Option<UnshieldedOffer>,
    actions:                     Vec<ContractAction>,
    dust_actions:                Option<DustActions>,
    ttl:                         Timestamp,
    binding_commitment:          B,
}
```

## Segment model

Every piece of a transaction belongs to a segment:

- **Segment 0 (guaranteed)**: always executes first. Contains the `guaranteed_offer` and the guaranteed parts of each intent (guaranteed unshielded offers, guaranteed contract transcripts, dust actions, fee payments). If anything in segment 0 fails, the entire transaction fails.
- **Segments 1..N (fallible)**: execute in order of segment ID after the guaranteed section. Each segment is atomic in isolation: if a fallible segment fails, only that segment is rolled back; the rest of the transaction can still succeed (`SucceedPartially`).

Intents and fallible offers carry a `segment_id: u16` that groups them. A single intent's contract actions can produce both guaranteed and fallible transcripts, which land in segment 0 and the intent's segment respectively.

## ZswapOffers

ZswapOffers handle **shielded** token transfers (privacy-preserving, using commitments and nullifiers). They exist at the transaction level, not inside contracts.

- `guaranteed_offer` (segment 0): always applied. If it can't be applied, the transaction fails entirely.
- `fallible_offer` (per segment): checked for applicability (valid Merkle trees, unspent nullifiers) during segment 0. Applied during their segment. If application fails, only that segment rolls back.

A ZswapOffer contains:
- `inputs`: shielded coin nullifiers (spending existing coins)
- `outputs`: shielded coin commitments (creating new coins)
- `transients`: coins that are both created and spent within the same transaction
- `deltas`: per-token-type balance changes

Contracts never create ZswapOffers. The off-chain wallet/transaction builder constructs them to satisfy constraints that contracts declare through their effects.

## Contract effects

When a contract executes (via a `ContractAction::Call`), it produces a transcript containing an `Effects` section. Effects are **constraints** that the transaction builder must satisfy with matching entries elsewhere in the same segment.

### Shielded token effects

| Effect | Direction | What it means |
|--------|-----------|---------------|
| `shielded_mints` | Contract -> User | Contract mints new shielded tokens. Compact: `sendShielded`. The wallet must include a matching coin commitment in a ZswapOffer output. |
| `claimed_shielded_receives` | User -> Contract | Contract claims coin commitments from ZswapOffer outputs. Compact: `receiveShielded`. Each claim must match exactly one commitment in a ZswapOffer in the same segment, associated with the same contract address. |
| `claimed_nullifiers` | User -> Contract | Contract claims nullifiers from ZswapOffer inputs. Each claimed nullifier must match exactly one nullifier in a ZswapOffer in the same segment, associated with the same contract address. |
| `claimed_shielded_spends` | Contract -> User | Contract claims coin commitments that exist in ZswapOffer outputs. At most one contract can claim any given commitment. |

### Unshielded token effects

| Effect | Direction | What it means |
|--------|-----------|---------------|
| `unshielded_inputs` | User -> Contract | Unshielded tokens consumed by the contract. Despite the name "input", this is an *output* from the transaction's perspective (value leaves the user). Must be matched by an unshielded offer output or another contract's unshielded output in the same segment. |
| `unshielded_outputs` | Contract -> User | Unshielded tokens produced by the contract. Despite the name "output", this is an *input* to the transaction (value enters the user). |
| `unshielded_mints` | Contract -> Ledger | Contract mints new unshielded tokens. The token type is derived from `hash(contract_address, pre_token)`. |

### Contract-to-contract effects

| Effect | What it means |
|--------|---------------|
| `claimed_contract_calls` | Contract declares that another contract call must exist in the same segment, identified by `(address, entry_point_hash, communication_commitment)`. |

## Effects check (1-to-1 matching)

The ledger enforces strict 1-to-1 existence constraints between effects and the rest of the transaction:

1. Every contract-associated nullifier in a ZswapOffer must be claimed by exactly one instance of the same contract in the same segment, and vice versa.
2. Every contract-associated coin commitment in a ZswapOffer must be claimed by exactly one `claimed_shielded_receives` from the same contract in the same segment, and vice versa.
3. Every `claimed_shielded_spends` must correspond to an existing commitment in a ZswapOffer in the same segment. At most one contract can claim any given commitment.
4. Every `claimed_contract_calls` must correspond to an actual contract call in the same segment.
5. Every `claimed_unshielded_spends` must match an unshielded offer output or a contract's `unshielded_inputs` in the same segment.

## Balancing

The transaction must balance per token type, per segment:

- Shielded balances come from ZswapOffer deltas + contract shielded mints.
- Unshielded balances come from unshielded offer inputs/outputs + contract unshielded inputs/outputs/mints.
- Fees (denominated in DUST) are accumulated across all segments and checked in segment 0.

Pedersen commitments enforce that the declared shielded balances match the actual ZswapOffer value commitments, without revealing amounts.

## Causal precedence (sequencing within a transaction)

When two segments call the same contract, or a contract calls another contract, causal precedence rules apply:

- If segments `a < b` call the same contract, then either `a` has no fallible transcript for that contract, or `b` has no guaranteed transcript for it.
- If contract `a` calls contract `b`, then `a` causally precedes `b`, meaning `b`'s execution must fit within `a`'s lifecycle (guaranteed call -> only guaranteed transcript; fallible call -> only fallible transcript).
- Causal precedence is transitive.

This ensures the guaranteed-before-fallible ordering is never violated.

## Key takeaway: contracts are reactive, not generative

Contracts cannot:
- Generate intents
- Create ZswapOffers
- Initiate transactions

Contracts can only:
- Execute within an intent's `actions` when called
- Produce effects (mints, claims, constraints) that the off-chain transaction builder must satisfy
- Call other contracts (but only within the same intent)

All transaction construction, balancing, proving, and submission happens off-chain.

## TTL window and the dev-genesis pitfall

Every intent carries a `ttl: Timestamp`. At validation the ledger enforces (spec lines 128-131, 246-248):

```rust
assert!(intent.ttl >= tblock && intent.ttl <= tblock + global_ttl);
```

— the TTL must be no earlier than the current block time and no further in the future than `global_ttl`. The SDK computes `intent.ttl = chain_tblock + global_ttl` from the wallet's view of the chain (`block_context.tblock` populated during sync); it does **not** use the client's system clock.

That's correct on any chain whose successive blocks track real time. It's a trap on the **local dev devnet**, where block 0 ships with a hardcoded `tblock` from months before wall clock (`midnightntwrk/midnight-node:0.22.1` genesis: 2025-08-05) but block 1+ uses the validator's real clock. If you build a transaction while the chain has only genesis, `intent.ttl` lands months in the past relative to the chain's view as soon as block 1 arrives — submission is rejected with chain custom error 182.

The SDK guards against this automatically: [`MidnightProvider::resync_wallet`](../crates/midnight-provider/src/provider.rs) calls [`wait_for_chain_ready`](../crates/midnight-provider/src/provider.rs) (polls the indexer until block height ≥ 1, max 60s, returns [`ProviderError::ChainNotReady`](../crates/midnight-provider/src/error.rs) on timeout). Every transfer and contract path goes through `resync_wallet`, so callers get the guard for free. On any chain past block 1 (mainnet, preprod, or any local devnet older than ~6s) the wait is a single warm indexer query and returns immediately.
