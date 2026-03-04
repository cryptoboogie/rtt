# pm-executor

Integration binary: wires together pm-data, pm-strategy, and rtt-core into a single pipeline.

## Running Tests

### Unit tests (no network)

```bash
cargo test -p pm-executor --lib
```

### Integration tests (some require network access to Polymarket WebSocket)

```bash
cargo test -p pm-executor --test '*'
```

### All tests

```bash
cargo test -p pm-executor
```
