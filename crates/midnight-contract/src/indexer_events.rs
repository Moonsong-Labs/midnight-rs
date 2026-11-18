//! The events that a contract emitted up to the latest block of the indexer.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Duration;

use midnight_helpers::mn_ledger::events::EventDetails;
use midnight_helpers::{DefaultDB, Event};
use midnight_indexer_client::{IndexerClient, IndexerError, SubscriptionClient};
use midnight_provider::TransactionHash;
use midnight_typed_state::tagged_deserialize;
use midnight_types::convert::IntoSdk;
use midnight_types::{BlockHash, ContractAddress};
use serde::Deserialize;

use crate::contract::Contract;
use crate::error::ContractError;
use crate::events::ContractEvent;
use crate::{format_contract_address, parse_contract_address};

/// The `contractEvents` subscription. It selects `raw`, the tagged ledger
/// `Event`, and not the typed fields, which the indexer decodes leniently.
const CONTRACT_EVENTS: &str = r#"
    subscription ContractEvents($filter: ContractEventFilter!) {
        contractEvents(filter: $filter) {
            raw
            transaction { hash block { hash height } }
        }
    }
"#;

/// How long the read waits for the next row, or for the end of the read.
/// The indexer answers the client's pings while it waits on the chain. So
/// only a bound on the rows ends a wait for a block that it lacks.
const ROW_TIMEOUT: Duration = Duration::from_secs(60);

/// The events that a contract emitted in one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEvents {
    /// The height of the block.
    pub height: u64,
    /// The hash of the block, the input of [`Contract::events_at`].
    pub hash: BlockHash,
    /// The events, in the order in which the indexer stored them.
    pub events: Vec<ContractEvent>,
}

/// The result of [`Contract::latest_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestEvents {
    /// Each block of the read that holds an event of the contract, by height.
    pub blocks: Vec<BlockEvents>,
    /// The first height that the read did not cover. Pass it as
    /// [`FromBlock::Height`] to read the events that follow.
    pub next_height: u64,
}

/// The first block of [`Contract::latest_events`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FromBlock {
    /// The indexer's latest block, so the read gives only the events of that
    /// block. A follower of new events starts here.
    Latest,
    /// The block at this height.
    Height(u64),
}

impl<P> Contract<P> {
    /// Get the events that this contract emitted from the block at `from`
    /// to the latest block of the indexer at `indexer_url`.
    ///
    /// The indexer runs each finalized transaction on its own copy of the
    /// ledger, and keeps the events of the segments that applied in that run.
    /// The node does not check that run. The indexer leaves out a transaction
    /// that Council, TechnicalCommittee, FederatedAuthority or the Scheduler
    /// dispatches. So the events of such a transaction are missing with no
    /// error. [`Self::events_at`] reads a block from the node and gives them
    /// when the block body holds a copy of the transaction. When the block
    /// ran the transaction from storage, as for a Scheduler call, `events_at`
    /// fails with [`ContractError::UnknownEvents`].
    ///
    /// The read covers every block from `from` to the indexer's latest block
    /// at the start of the read, and holds all their events in memory. To
    /// follow the contract, read again from
    /// [`next_height`](LatestEvents::next_height). When the indexer's latest
    /// block is below `from`, the read gives no events and keeps `from` as
    /// the next height.
    ///
    /// It decodes each [`ContractEvent`] from the ledger event that the
    /// indexer stores, not from the indexer's decoded fields. It uses the
    /// indexer's `contractEvents` subscription, which the indexer marks
    /// `@beta`, so it needs an indexer of the 4.4 line. `indexer_url` takes
    /// the forms that `WalletBuilder::sync` takes.
    ///
    /// # Errors
    ///
    /// - [`ContractError::Indexer`] when the read fails. `retryable` is true
    ///   in these cases:
    ///   - the WebSocket connection fails;
    ///   - the connection closes before the indexer completes the read;
    ///   - the indexer sends no row for 60 s;
    ///   - the latest-block query fails before a response, or gets a 5xx
    ///     status.
    ///
    ///   `retryable` is false in these cases:
    ///   - the indexer answers with a GraphQL error, for example a refusal
    ///     from an indexer with no `contractEvents`, or its own
    ///     `Internal Server Error`;
    ///   - the latest-block query times out, or gets another status;
    ///   - the indexer's latest height is negative;
    ///   - a row is not a ledger 9 event of this contract in the range.
    /// - [`ContractError::InvalidContractAddress`] when the handle's address
    ///   does not parse.
    pub async fn latest_events(
        &self,
        indexer_url: &str,
        from: FromBlock,
    ) -> Result<LatestEvents, ContractError> {
        let address = parse_contract_address(self.address())?;
        read(indexer_url, &address, from).await
    }
}

async fn read(
    indexer_url: &str,
    address: &ContractAddress,
    from: FromBlock,
) -> Result<LatestEvents, ContractError> {
    let latest = IndexerClient::new(indexer_url)
        .map_err(indexer_error)?
        .get_block(None)
        .await
        .map_err(indexer_error)?
        .map(|block| {
            u64::try_from(block.height)
                .map_err(|_| disagrees(format!("a negative latest height {}", block.height)))
        })
        .transpose()?;
    let (range, next_height) = covered(from, latest);
    let Some(range) = range else {
        return Ok(LatestEvents {
            blocks: Vec::new(),
            next_height,
        });
    };
    let filter = serde_json::json!({
        "contractAddress": format_contract_address(address),
        "fromBlock": range.start(),
        "toBlock": range.end(),
    });
    let mut rows = SubscriptionClient::new(indexer_url)
        .subscribe::<Rows>(CONTRACT_EVENTS, serde_json::json!({ "filter": filter }))
        .await
        .map_err(indexer_error)?;
    let mut events = Vec::new();
    loop {
        let row = match tokio::time::timeout(ROW_TIMEOUT, rows.next()).await {
            Err(_) => {
                return Err(ContractError::Indexer {
                    retryable: true,
                    source: format!("the indexer sent no row for {ROW_TIMEOUT:?}").into(),
                });
            }
            Ok(None) => break,
            Ok(Some(row)) => row.map_err(indexer_error)?,
        };
        events.push(decode_row(row, address)?);
    }
    Ok(LatestEvents {
        blocks: group_by_block(events, &range)?,
        next_height,
    })
}

/// The heights that a read from `from` covers when the indexer's latest
/// block is at `latest`, and the first height after them.
fn covered(from: FromBlock, latest: Option<u64>) -> (Option<RangeInclusive<u64>>, u64) {
    match (from, latest) {
        (FromBlock::Latest, None) => (None, 0),
        (FromBlock::Height(from), None) => (None, from),
        (FromBlock::Latest, Some(latest)) => (Some(latest..=latest), latest + 1),
        (FromBlock::Height(from), Some(latest)) if from <= latest => {
            (Some(from..=latest), latest + 1)
        }
        (FromBlock::Height(from), Some(_)) => (None, from),
    }
}

/// The events by block, in height order. Each height must lie in `range` and
/// have one hash.
fn group_by_block(
    events: Vec<(u64, BlockHash, ContractEvent)>,
    range: &RangeInclusive<u64>,
) -> Result<Vec<BlockEvents>, ContractError> {
    let mut blocks = BTreeMap::<u64, BlockEvents>::new();
    for (height, hash, event) in events {
        if !range.contains(&height) {
            return Err(disagrees(format!(
                "a row of block {height}, outside {range:?}"
            )));
        }
        let block = blocks.entry(height).or_insert_with(|| BlockEvents {
            height,
            hash,
            events: Vec::new(),
        });
        if block.hash != hash {
            return Err(disagrees(format!("two hashes for block {height}")));
        }
        block.events.push(event);
    }
    Ok(blocks.into_values().collect())
}

#[derive(Debug, Deserialize)]
struct Rows {
    #[serde(rename = "contractEvents")]
    row: Row,
}

#[derive(Debug, Deserialize)]
struct Row {
    raw: String,
    transaction: RowTransaction,
}

#[derive(Debug, Deserialize)]
struct RowTransaction {
    hash: String,
    block: RowBlock,
}

#[derive(Debug, Deserialize)]
struct RowBlock {
    hash: String,
    height: u64,
}

fn indexer_error(error: IndexerError) -> ContractError {
    ContractError::Indexer {
        retryable: error.is_retryable(),
        source: Box::new(error),
    }
}

fn disagrees(reason: String) -> ContractError {
    ContractError::Indexer {
        retryable: false,
        source: reason.into(),
    }
}

/// Decode `raw`, the tagged ledger `Event` of the row, and check it against
/// the read and the row's other fields.
fn decode_row(
    Rows { row }: Rows,
    address: &ContractAddress,
) -> Result<(u64, BlockHash, ContractEvent), ContractError> {
    let raw = hex::decode(&row.raw).map_err(|e| disagrees(format!("raw is not hex: {e}")))?;
    let event: Event<DefaultDB> = tagged_deserialize(raw.as_slice())
        .map_err(|e| disagrees(format!("raw is not a ledger 9 event: {e}")))?;
    let EventDetails::ContractLog {
        address: emitter,
        entry_point,
        logged_item,
    } = event.content
    else {
        return Err(disagrees("a ledger event that is not a ContractLog".into()));
    };
    let emitter: ContractAddress = emitter.into_sdk();
    if emitter != *address {
        return Err(disagrees(format!(
            "an event of another contract, {}",
            format_contract_address(&emitter)
        )));
    }
    let transaction_hash = TransactionHash::from(event.source.transaction_hash.0.0);
    if !row
        .transaction
        .hash
        .eq_ignore_ascii_case(&transaction_hash.to_string())
    {
        return Err(disagrees(format!(
            "the event names transaction {transaction_hash}, the row {}",
            row.transaction.hash
        )));
    }
    let block_hash = hex::decode(&row.transaction.block.hash)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(BlockHash::from)
        .ok_or_else(|| {
            disagrees(format!(
                "a block hash that is not 32 bytes of hex: {}",
                row.transaction.block.hash
            ))
        })?;
    Ok((
        row.transaction.block.height,
        block_hash,
        ContractEvent {
            transaction_hash,
            entry_point: entry_point.0,
            item: logged_item,
        },
    ))
}

#[cfg(test)]
mod tests {
    use midnight_helpers::mn_ledger::events::EventSource;
    use midnight_typed_state::{InMemoryDB, StateValue, tagged_serialize};
    use midnight_types::HashOutput;

    use super::*;
    use crate::runtime::{LogEventType, VersionedLogItem};

    /// It catches these mistakes:
    /// - a `Latest` that starts at block 0 (the whole history);
    /// - a next height of the last block (the read gives that block twice);
    /// - a subscription with `fromBlock` above `toBlock` (the indexer refuses
    ///   it at every poll of a follower at the tip);
    /// - a next height below `from` when the indexer trails the caller;
    /// - a next height other than 0 when the indexer holds no block.
    #[test]
    fn a_read_covers_its_start_to_the_indexer_latest_block() {
        let cases = [
            (FromBlock::Latest, Some(90), (Some(90..=90), 91)),
            (FromBlock::Height(10), Some(90), (Some(10..=90), 91)),
            (FromBlock::Height(90), Some(90), (Some(90..=90), 91)),
            (FromBlock::Height(95), Some(90), (None, 95)),
            (FromBlock::Height(5), None, (None, 5)),
            (FromBlock::Latest, None, (None, 0)),
        ];
        for (from, latest, expected) in cases {
            assert_eq!(covered(from, latest), expected, "{from:?}, {latest:?}");
        }
    }

    fn row(emitter: u8, transaction: u8, row_transaction: u8) -> Rows {
        let event: Event<DefaultDB> = Event {
            source: EventSource {
                transaction_hash: midnight_helpers::TransactionHash(HashOutput([transaction; 32])),
                logical_segment: 0,
                physical_segment: 1,
            },
            content: EventDetails::ContractLog {
                address: midnight_helpers::ContractAddress(HashOutput([emitter; 32])),
                entry_point: midnight_helpers::EntryPointBuf(b"record".to_vec()),
                logged_item: VersionedLogItem {
                    version: 1,
                    event_type: LogEventType::Misc,
                    data: StateValue::Null,
                },
            },
        };
        let mut raw = Vec::new();
        tagged_serialize(&event, &mut raw).expect("an event serializes");
        Rows {
            row: Row {
                raw: hex::encode(raw),
                transaction: RowTransaction {
                    hash: hex::encode([row_transaction; 32]),
                    block: RowBlock {
                        hash: hex::encode([9; 32]),
                        height: 7,
                    },
                },
            },
        }
    }

    /// It catches `raw` read as a `VersionedLogItem`. It also catches a read
    /// that trusts the indexer's address filter, or its join of an event to a
    /// transaction.
    #[test]
    fn a_row_that_disagrees_with_the_read_is_an_error() {
        let address = ContractAddress(HashOutput([1; 32]));
        let (height, hash, event) =
            decode_row(row(1, 2, 2), &address).expect("an event of the contract");
        assert_eq!(
            (
                height,
                hash,
                event.transaction_hash,
                event.entry_point.as_slice()
            ),
            (
                7,
                BlockHash::from([9; 32]),
                TransactionHash::from([2; 32]),
                b"record".as_slice()
            )
        );
        for (name, other) in [("address", row(3, 2, 2)), ("transaction", row(1, 2, 4))] {
            assert!(
                matches!(
                    decode_row(other, &address),
                    Err(ContractError::Indexer {
                        retryable: false,
                        ..
                    })
                ),
                "{name}"
            );
        }
    }

    fn event(byte: u8) -> ContractEvent {
        ContractEvent {
            transaction_hash: TransactionHash::from([byte; 32]),
            entry_point: b"record".to_vec(),
            item: VersionedLogItem::<InMemoryDB> {
                version: 1,
                event_type: LogEventType::Misc,
                data: StateValue::Null,
            },
        }
    }

    /// It catches a grouping that trusts the row order, which gives two
    /// `BlockEvents` for one height. It also catches a read that keeps a row
    /// outside the range, which a read of the next range gives again.
    #[test]
    fn rows_group_by_height_inside_the_range() {
        let (a, b) = (BlockHash::from([5; 32]), BlockHash::from([7; 32]));
        let blocks = group_by_block(
            vec![(7, b, event(1)), (5, a, event(2)), (7, b, event(3))],
            &(0..=9),
        );
        assert_eq!(
            blocks.expect("rows inside the range"),
            [
                BlockEvents {
                    height: 5,
                    hash: a,
                    events: vec![event(2)],
                },
                BlockEvents {
                    height: 7,
                    hash: b,
                    events: vec![event(1), event(3)],
                },
            ]
        );
        for (name, rows) in [
            ("a row before the range", vec![(1, a, event(1))]),
            ("a row past the range", vec![(10, b, event(1))]),
            (
                "two hashes for one height",
                vec![(7, a, event(1)), (7, b, event(2))],
            ),
        ] {
            assert!(
                matches!(
                    group_by_block(rows, &(2..=9)),
                    Err(ContractError::Indexer {
                        retryable: false,
                        ..
                    })
                ),
                "{name}"
            );
        }
    }

    /// A read from a mock indexer that holds one event, and ends the
    /// subscription with `complete` when `complete` is true, else with a
    /// close frame.
    async fn read_one_row(complete: bool) -> Result<LatestEvents, ContractError> {
        use midnight_indexer_client::testutil::{
            accept_subscriber, bind, next_json, read_http_request, send_json, send_next,
            write_json_response,
        };

        let (listener, url) = bind().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(read_http_request(&mut stream).await);
            let latest = serde_json::json!({ "data": { "block": { "hash": "09".repeat(32), "height": 9 } } });
            write_json_response(&mut stream, "200 OK", &latest.to_string()).await;

            let (mut ws, sub) = accept_subscriber(&listener).await;
            let filter = &sub["payload"]["variables"]["filter"];
            assert_eq!(
                (&filter["fromBlock"], &filter["toBlock"]),
                (&serde_json::json!(0), &serde_json::json!(9))
            );
            let Rows { row } = row(1, 2, 2);
            let data = serde_json::json!({ "contractEvents": {
                "raw": row.raw,
                "transaction": {
                    "hash": row.transaction.hash,
                    "block": { "hash": row.transaction.block.hash, "height": row.transaction.block.height },
                },
            }});
            send_next(&mut ws, &sub, data).await;
            if complete {
                send_json(
                    &mut ws,
                    &serde_json::json!({ "type": "complete", "id": sub["id"] }),
                )
                .await;
            } else {
                ws.close(None).await.unwrap();
            }
            while next_json(&mut ws).await.is_some() {}
        });
        let read = read(
            &url,
            &ContractAddress(HashOutput([1; 32])),
            FromBlock::Height(0),
        )
        .await;
        server.abort();
        read
    }

    /// It catches a read that stops at the first error and returns the rows
    /// before it, so that a cut read passes as a complete one. It also
    /// catches a filter with no `toBlock`, which the indexer never completes.
    #[tokio::test]
    async fn only_a_completed_subscription_gives_the_events() {
        let read = read_one_row(true).await.expect("a completed read");
        assert_eq!(
            (
                read.blocks
                    .iter()
                    .map(|block| (block.height, block.events.len()))
                    .collect::<Vec<_>>(),
                read.next_height
            ),
            (vec![(7, 1)], 10)
        );
        assert!(matches!(
            read_one_row(false).await,
            Err(ContractError::Indexer {
                retryable: true,
                ..
            })
        ));
    }
}
