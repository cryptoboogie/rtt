# rtt-core

Low-latency execution engine: warm HTTP/2 connection pool, CLOB order signing, pre-signed order dispatch, nanosecond timestamp instrumentation.

## Running Tests

### Unit tests (no network)

```bash
cargo test -p rtt-core --lib
```

### Integration tests (requires network access to clob.polymarket.com)

```bash
cargo test -p rtt-core --test '*'
```

### All tests

```bash
cargo test -p rtt-core
```

### Ignored tests (sends real orders — requires credentials)

```bash
cargo test -p rtt-core -- --ignored
```

Only `test_clob_end_to_end_pipeline` is ignored. It POSTs a real order and requires `POLY_API_KEY`, `POLY_SECRET`, `POLY_PASSPHRASE`, `POLY_ADDRESS`, and `POLY_PRIVATE_KEY` environment variables.
