# pm-data

WebSocket data pipeline: connects to Polymarket's market WebSocket, maintains local order books, and broadcasts snapshots.

## Running Tests

### Unit tests (no network)

```bash
cargo test -p pm-data --lib
```

### Integration tests (requires network access to Polymarket WebSocket)

```bash
cargo test -p pm-data --test '*'
```

### All tests

```bash
cargo test -p pm-data
```
