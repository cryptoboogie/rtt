use std::time::Duration;

use pm_data::types::WsMessage;
use pm_data::ws::WsClient;
use pm_data::OrderBookManager;
use tokio::time::timeout;

// A real active asset on Polymarket (this is a commonly traded market)
// If this specific asset becomes inactive, any active token_id can be substituted
// Active high-volume market token — update if market resolves
const TEST_ASSET_ID: &str =
    "48825140812430902098404528620382945035793471220915259967486864813738884055220";

#[tokio::test]
async fn connect_subscribe_receive_book_snapshot() {
    let mut client = WsClient::new(vec![TEST_ASSET_ID.to_string()], 100);
    let mut rx = client.subscribe();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    // Wait for a book event (first message should be a book snapshot due to initial_dump)
    let result = timeout(Duration::from_secs(15), async {
        loop {
            match rx.recv().await {
                Ok(WsMessage::Book(book)) => {
                    assert_eq!(book.asset_id, TEST_ASSET_ID);
                    assert!(!book.bids.is_empty() || !book.asks.is_empty());
                    return book;
                }
                Ok(_) => continue, // skip non-book messages
                Err(e) => panic!("Receive error: {e}"),
            }
        }
    })
    .await;

    assert!(result.is_ok(), "Timed out waiting for book snapshot");
    let book = result.unwrap();
    println!(
        "Received book: {} bids, {} asks, hash={:?}",
        book.bids.len(),
        book.asks.len(),
        book.hash
    );

    handle.abort();
}

#[tokio::test]
async fn pipeline_updates_orderbook_from_ws() {
    let mut client = WsClient::new(vec![TEST_ASSET_ID.to_string()], 100);
    let mut rx = client.subscribe();
    let order_books = OrderBookManager::new();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    // Process messages until we get a book update applied
    let result = timeout(Duration::from_secs(15), async {
        loop {
            match rx.recv().await {
                Ok(WsMessage::Book(book)) => {
                    order_books.apply_book_update(&book);
                    return order_books.get_snapshot(TEST_ASSET_ID).unwrap();
                }
                Ok(WsMessage::PriceChange(pc)) => {
                    let ts: u64 = pc.timestamp.parse().unwrap_or(0);
                    for entry in &pc.price_changes {
                        order_books.apply_price_change(entry, ts);
                    }
                    if let Some(snap) = order_books.get_snapshot(TEST_ASSET_ID) {
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

    assert!(result.is_ok(), "Timed out waiting for order book update");
    let snap = result.unwrap();
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
    let mut client = WsClient::new(vec![TEST_ASSET_ID.to_string()], 100);
    let mut rx = client.subscribe();

    let handle = tokio::spawn(async move {
        client.run().await;
    });

    // Receive messages for 15 seconds to verify keepalive works
    let result = timeout(Duration::from_secs(30), async {
        let start = std::time::Instant::now();
        let mut msg_count = 0u64;
        let mut last_msg = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(15) {
            match timeout(Duration::from_secs(14), rx.recv()).await {
                Ok(Ok(_)) => {
                    msg_count += 1;
                    last_msg = std::time::Instant::now();
                }
                Ok(Err(e)) => {
                    // Lagged is OK, closed is not
                    if matches!(
                        e,
                        tokio::sync::broadcast::error::RecvError::Closed
                    ) {
                        panic!("Channel closed during keepalive test after {msg_count} messages");
                    }
                }
                Err(_) => {
                    panic!(
                        "No message for 14s — keepalive likely failed. Last msg {:?} ago",
                        last_msg.elapsed()
                    );
                }
            }
        }
        msg_count
    })
    .await;

    assert!(result.is_ok(), "Timed out during keepalive test");
    let count = result.unwrap();
    println!("Received {count} messages over 15s keepalive test");
    assert!(count > 0, "Should have received at least one message");

    handle.abort();
}
