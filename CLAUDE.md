# RTT — Low-Latency Polymarket Execution

## Key Documents

| Document | Purpose |
|---|---|
| `ARCHITECTURE.md` | **Read this first.** System design, components, data flow, design decisions, glossary. |
| `IMPLEMENTATION_LOG.md` | Session-by-session history of all work. Use as reference for past decisions and deviations. |
| `config.toml` | Runtime configuration. All fields documented in ARCHITECTURE.md § Configuration. |

## Way of Working

1. **Break plans into big tasks** — each big task represents a meaningful capability milestone
2. **Break tasks into sub-tasks** — sub-tasks are atomic units of work
3. **TDD for every sub-task** — write a failing test first, then write the minimal code to pass the test(s)
4. **Once the test passes, move on** — do not gold-plate; proceed to the next sub-task immediately
5. **Do not stop until all sub-tasks are finished** — unless there is a fatal blocking issue
6. **Log every sub-task** — for each completed sub-task, append an entry to `IMPLEMENTATION_LOG.md` recording files changed, tests run, commit message, any deviations from the plan, reasons for decisions, and any notable info encountered in the workflow
7. **When finished, run and verify ALL project test suites pass (unit and integration)**

## Running Tests

```bash
cargo test --workspace              # All tests (196 pass, 1 ignored)
cargo test --workspace --lib        # Unit tests only (no network)
cargo test -p <crate>               # Single crate: rtt-core, pm-data, pm-strategy, pm-executor
```

**The ignored test (`test_clob_end_to_end_pipeline`) sends a real order and costs real money.** It requires `POLY_*` env vars. Do not run it without explicit user authorization.

## References

- Polymarket API docs: https://docs.polymarket.com/market-data/overview
- Rust CLOB client: https://github.com/Polymarket/rs-clob-client
