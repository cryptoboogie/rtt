# Session 3: Strategy Framework — Implementation Log

## Sub-task 1: Create pm-strategy Cargo crate with shared types
- **Files changed**: `Cargo.toml`, `src/lib.rs`, `src/types.rs`, `tests/types_test.rs`
- **Tests run**: 5 passed (types serde roundtrip, snapshot construction, trade event, enum variants)
- **Commit**: initial crate with shared types (TriggerMessage, OrderBookSnapshot, PriceLevel, TradeEvent, Side, OrderType)

## Sub-task 2: Define Strategy trait
- **Files changed**: `src/strategy.rs`, `src/lib.rs`, `tests/strategy_trait_test.rs`
- **Tests run**: 8 passed (+3 new: mock strategy fires once, on_trade returns None, Send+Sync assertion)
- **Commit**: Strategy trait with on_book_update, on_trade, name methods

## Sub-task 3: Implement ThresholdStrategy
- **Files changed**: `src/threshold.rs`, `src/lib.rs`, `tests/threshold_test.rs`
- **Tests run**: 16 passed (+8 new: fire on ask crossing down, fire on bid crossing up, exact price, empty book, wrong asset, name, trigger ID increment, no fire below threshold)
- **Commit**: ThresholdStrategy — fires when best_ask <= threshold (Buy) or best_bid >= threshold (Sell)

## Sub-task 4: Implement SpreadStrategy
- **Files changed**: `src/spread.rs`, `src/lib.rs`, `tests/spread_test.rs`
- **Tests run**: 25 passed (+9 new: fire on narrow spread, no fire on wide spread, buy uses ask price, sell uses bid price, empty book, one-sided book, wrong asset, name, trigger ID increment)
- **Commit**: SpreadStrategy — fires when bid-ask spread narrows below max_spread

## Sub-task 5: Build Strategy runner
- **Files changed**: `src/runner.rs`, `src/lib.rs`, `tests/runner_test.rs`
- **Tests run**: 29 passed (+4 new: runner forwards trigger from threshold, forwards multiple from spread, handles empty channel, processes sequence of 10 mock snapshots)
- **Commit**: StrategyRunner — async loop receiving snapshots via mpsc channel, calling strategy, forwarding triggers

## Sub-task 6: Add TOML configuration loading
- **Files changed**: `src/config.rs`, `src/lib.rs`, `tests/config_test.rs`
- **Tests run**: 37 passed (+8 new: parse threshold/spread TOML, build strategies from config, unknown strategy error, missing param error, roundtrip serialize, load from file)
- **Commit**: StrategyConfig with from_file() and build_strategy() — loads strategy name, params, token_id, side, size, order_type from TOML

## Sub-task 7: Add backtesting mode
- **Files changed**: `src/backtest.rs`, `src/lib.rs`, `tests/backtest_test.rs`
- **Tests run**: 43 passed (+6 new: threshold finds triggers in replay, spread finds triggers, empty snapshots, no matches, load from JSON file, sequential trigger IDs)
- **Commit**: BacktestRunner — replays Vec<OrderBookSnapshot> through any Strategy, collects triggers, loads snapshots from JSON

## Summary
- **Total tests**: 43 passing, 0 warnings
- **Modules**: types, strategy (trait), threshold, spread, runner, config, backtest
- **All success criteria met**:
  - ThresholdStrategy correctly fires trigger at configured price
  - SpreadStrategy correctly fires trigger at configured spread
  - Strategy runner processes sequences of mock snapshots via async channels
  - Config loads from TOML file and builds concrete strategies
  - Backtesting replays saved snapshots through strategy
