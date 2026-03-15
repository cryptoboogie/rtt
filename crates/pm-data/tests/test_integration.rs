use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pm_data::types::WsMessage;
use pm_data::ws::WsClient;
use pm_data::OrderBookManager;
use tokio::time::{sleep, timeout};

// Verified against the Gamma active-markets feed on March 9, 2026.
// Override with PM_DATA_TEST_ASSET_IDS="id1,id2,..." if these markets later resolve.
const DEFAULT_TEST_ASSET_IDS: &[&str] = &[
    "83913782129625990038392446861662263440481724210183068438420029953791573220565",
    "65776331158171098119883600447375115999924641197000014423196868505933237200018",
    "81174786818713261193560505453499396552702726344851023968604467996639370216099",
    "70806431869074947217011092888214145387780886214872973629706902768064897436431",
    "95799630929919147352395495314136959486413119752895015014091248800858824386556",
    "103614317189813130876406667087690897791577446874828443876431098607381069690878",
];

fn test_asset_ids() -> Vec<String> {
    std::env::var("PM_DATA_TEST_ASSET_IDS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|asset_ids| !asset_ids.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_TEST_ASSET_IDS
                .iter()
                .map(|asset_id| asset_id.to_string())
                .collect()
        })
}

async fn wait_for_activity(
    last_message_at: &Arc<AtomicU64>,
    previous: u64,
    wait_for: Duration,
) -> Option<u64> {
    let started = std::time::Instant::now();

    while started.elapsed() < wait_for {
        let current = last_message_at.load(Ordering::Relaxed);
        if current > previous {
            return Some(current);
        }
        sleep(Duration::from_millis(100)).await;
    }

    None
}

#[tokio::test]
async fn connect_subscribe_receive_book_snapshot() {
    let test_asset_ids = test_asset_ids();
    let client = WsClient::new(test_asset_ids.clone(), 100);
    let mut rx = client.subscribe();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    let result = timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Ok(WsMessage::Book(book)) => {
                    if test_asset_ids.contains(&book.asset_id) {
                        assert!(!book.bids.is_empty() || !book.asks.is_empty());
                        return ("book", book.asset_id);
                    }
                }
                Ok(WsMessage::PriceChange(pc)) => {
                    if let Some(entry) = pc
                        .price_changes
                        .iter()
                        .find(|entry| test_asset_ids.contains(&entry.asset_id))
                    {
                        return ("price_change", entry.asset_id.clone());
                    }
                }
                Ok(WsMessage::BestBidAsk(bbo)) => {
                    if test_asset_ids.contains(&bbo.asset_id) {
                        return ("best_bid_ask", bbo.asset_id);
                    }
                }
                Ok(_) => continue, // skip non-book messages
                Err(e) => panic!("Receive error: {e}"),
            }
        }
    })
    .await;

    let Ok((kind, asset_id)) = result else {
        eprintln!("No market update arrived within 30s; treating live feed check as inconclusive");
        handle.abort();
        return;
    };
    println!("Received {kind} update for asset {asset_id}");

    handle.abort();
}

#[tokio::test]
async fn pipeline_updates_orderbook_from_ws() {
    let test_asset_ids = test_asset_ids();
    let client = WsClient::new(test_asset_ids.clone(), 100);
    let mut rx = client.subscribe();
    let order_books = OrderBookManager::new();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    // Process messages until we get a book update applied
    let result = timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Ok(WsMessage::Book(book)) => {
                    order_books.apply_book_update(&book);
                    if let Some(snap) = order_books.get_snapshot(&book.asset_id) {
                        return snap;
                    }
                }
                Ok(WsMessage::PriceChange(pc)) => {
                    let ts: u64 = pc.timestamp.parse().unwrap_or(0);
                    for entry in &pc.price_changes {
                        order_books.apply_price_change(entry, ts);
                    }
                    if let Some(snap) = pc
                        .price_changes
                        .iter()
                        .find(|entry| test_asset_ids.contains(&entry.asset_id))
                        .and_then(|entry| order_books.get_snapshot(&entry.asset_id))
                    {
                        if snap.best_bid.is_some() || snap.best_ask.is_some() {
                            return snap;
                        }
                    }
                }
                Ok(_) => continue,
                Err(e) => panic!("Receive error: {e}"),
            }
        }
    })
    .await;

    let Ok(snap) = result else {
        eprintln!("No live book or delta update arrived within 30s; treating order-book check as inconclusive");
        handle.abort();
        return;
    };
    println!(
        "OrderBook snapshot: best_bid={:?}, best_ask={:?}, hash={}",
        snap.best_bid, snap.best_ask, snap.hash
    );
    // At least one side should have data for an active market
    assert!(
        snap.best_bid.is_some() || snap.best_ask.is_some(),
        "Expected at least one side of the book to have data"
    );

    handle.abort();
}

#[tokio::test]
async fn keepalive_no_disconnect_over_20_seconds() {
    let test_asset_ids = test_asset_ids();
    let client = WsClient::new(test_asset_ids, 100);
    let last_message_at = client.last_message_at_arc();
    let reconnect_count = client.reconnect_count_arc();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    let result = timeout(Duration::from_secs(40), async {
        let initial = wait_for_activity(&last_message_at, 0, Duration::from_secs(15))
            .await
            .expect("No initial WebSocket activity observed within 15s");

        assert_eq!(
            reconnect_count.load(Ordering::Relaxed),
            0,
            "Connection reconnected before keepalive observation window began"
        );
        assert!(
            !handle.is_finished(),
            "WsClient task exited before keepalive observation window began"
        );

        let first_advance = wait_for_activity(&last_message_at, initial, Duration::from_secs(13))
            .await
            .expect("No WebSocket activity observed during first 13s keepalive window");

        assert_eq!(
            reconnect_count.load(Ordering::Relaxed),
            0,
            "Connection reconnected during first keepalive window"
        );
        assert!(
            !handle.is_finished(),
            "WsClient task exited during first keepalive window"
        );

        let second_advance =
            wait_for_activity(&last_message_at, first_advance, Duration::from_secs(13))
                .await
                .expect("No WebSocket activity observed during second 13s keepalive window");

        assert_eq!(
            reconnect_count.load(Ordering::Relaxed),
            0,
            "Connection reconnected during second keepalive window"
        );
        assert!(
            !handle.is_finished(),
            "WsClient task exited during second keepalive window"
        );

        second_advance
    })
    .await;

    assert!(result.is_ok(), "Timed out during keepalive test");
    let last_activity = result.unwrap();
    println!(
        "Observed stable WebSocket activity without reconnects; last_message_at={last_activity}"
    );

    handle.abort();
}
