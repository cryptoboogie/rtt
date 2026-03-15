# BTC 5m Wallet Strategy Spec

## Scope

This spec is for the wallet's BTC 5-minute Polymarket behavior only.

Data used:

- Polymarket wallet activity rows
- Polymarket public market trade tapes as a proxy for market-wide prints
- Binance `aggTrades` as the reference timing tape

Coverage:

- First BTC 5m trade day: `2026-02-12`
- Last BTC 5m trade day: `2026-03-11`
- Markets analyzed: `7,083`
- Markets skipped because the next-day Binance close was unavailable: `1`

## Bottom Line

High-confidence conclusion:

- This is a schedule-driven BTC 5-minute market-open strategy.
- The wallet is not mainly a simple BTC direction predictor.
- The core edge is early acquisition of mispriced binary inventory in the first `5-21s` after open, usually around `7s`.
- The strategy has two major paired modes and one smaller one-sided continuation mode.

Most likely mental model:

- Mode A: paired inventory plus active recycling
- Mode B: paired inventory plus carry to settlement
- Mode C: smaller deliberate one-sided continuation bet

The tweet-style story of "wait for `0.12%`, then fire after `23s`" is not the right spec for BTC 5m.

## /todo Implementation Priorities Added On 2026-03-14

/todo 1: Make the live runtime match the strategy mechanics described by the analysis.

- add a Binance `BTCUSDT` trade or agg-trade websocket as a first-class live input
- capture a per-market open reference price at each `5m` boundary
- maintain short-horizon Binance continuation / reversal state
- replace the single fixed-size pair-entry model with `probe -> verify -> burst / pair / cleanup`
- implement mode-aware sizing rather than a single `pair_budget_usd`
- add Binance-informed first-side selection and veto logic
- either implement the one-sided continuation branch explicitly or disable it deliberately in config

/todo 2: Implement a separate small-account risk mode for the initial `$180` bankroll deployment.

- do not treat the historical wallet sizing as directly portable
- enforce `max_gross_deployed_per_market`, `max_unpaired_exposure_usd`, and `max_cleanup_loss_usd` separately
- start with pair-first behavior and disable or tightly cap one-sided continuation
- stop escalating if the second side cannot be acquired quickly and cheaply
- make cleanup a core transition, not an emergency fallback

These `/todo` items are implementation tasks, not requests for more analysis. The analysis sections below explain why they exist and what parameter ranges they should use.

## High-Confidence Findings

### 1. Universe And Timing

- The wallet trades almost all BTC 5-minute markets once the product becomes active.
- `97.2%` of analyzed markets were dual-sided.
- Median first entry offset was `7s`.
- `p10` entry offset was `5s`.
- `p90` entry offset was `21s`.

Interpretation:

- The bot is keyed to the market-open schedule, not to a delayed random trigger.

### 1A. Trade Structure And Capital Reuse

The BTC 5m strategy should be thought of as a repeating market micro-cycle, not as a set of independent swing bets.

Observed structure:

- one BTC 5m market opens every `300s`
- the strategy enters in the first `5-21s`, usually around `7s`
- it may buy one side first, then the opposite side a few seconds later
- it may sell inventory before settlement or carry it to settlement
- because the product resolves every `5m`, capital gets recycled very quickly

Implementation implication:

- gross daily notional is much larger than bankroll
- sizing and cleanup logic matter as much as entry timing
- the runtime should model a trade as a sequence of legs, not as one atomic "buy pair" event

### 2. Paired Inventory Is The Main Engine

- `6,885 / 7,083` markets were dual-sided.
- Full-history PnL split:
  - `paired_no_sells`: `1,787` markets, `$171,678.99`, `53.8%` of total PnL
  - `paired_with_sells`: `5,098` markets, `$133,589.85`, `41.9%` of total PnL
  - `single_side_only`: `192` markets, `$14,791.99`, `4.6%` of total PnL

Interpretation:

- The strategy is primarily a paired inventory strategy.
- One-sided markets matter, but they are a small secondary branch.

### 3. Contemporaneous Pair Capture Is Real

Using the wallet's own buy timestamps only, restricted to the first `30s` after open:

- In dual-sided markets, the best achieved pair sum was below `$1` within `2s` in `37.9%` of all dual markets.
- The best achieved pair sum was below `$1` within `5s` in `43.5%` of all dual markets.
- Median best same-window pair sum was about `0.95` within `5s`.
- Median pair completion offset was about `17s`.

Important nuance:

- These rates are conservative because they require the wallet itself to have actually bought both sides within the same short window.
- This is much stronger than the earlier hindsight-style "minimum Up plus minimum Down over the whole market" statistic.

Interpretation:

- The bot is genuinely capturing contemporaneous binary mispricing, not just accidentally touching both sides at unrelated times.

### 4. There Are Two Distinct Paired Modes

#### Paired With Sells

- `5,098` markets
- Median second-side delay: `10s`
- Median first pair sum: `0.98`
- `84.1%` had same-window pair sum below `$1` within `2s`
- Median aligned close move: basically `0`

Interpretation:

- This looks like the more microstructure-oriented mode.
- It builds the pair quickly and recycles inventory with lower directional exposure.

#### Paired No Sells

- `1,787` markets
- Median second-side delay: `18s`
- Median first pair sum: `0.95`
- `78.8%` had same-window pair sum below `$1` within `2s`
- Median aligned close move: `+3.93 bps`
- Higher per-market PnL than the sell-active mode

Interpretation:

- This looks like a cheaper-basis carry mode.
- The bot still pairs, but then often keeps inventory into settlement rather than actively recycling it.

### 4A. Sizing / Probe-And-Burst

The observed sizing behavior is staged.

The first fill row is not a good sizing proxy because Polymarket executions are fragmented across many rows. The better sizing proxy is the first `1s` burst after the initial buy.

Observed BTC 5m sizing:

- median first buy row: about `$13.44`
- median first `1s` burst: about `$52.17`
- median total buy cost per market: about `$1,213.92`

By mode:

- `paired_with_sells`
  - median first `1s` burst: about `$53.94`
  - median total buy cost: about `$1,494.80`
- `paired_no_sells`
  - median first `1s` burst: about `$49.17`
  - median total buy cost: about `$688.39`
- `single_side_only`
  - median first `1s` burst: about `$40.95`
  - median total buy cost: about `$464.98`

Interpretation:

- the bot appears to start with a modest probe clip
- it then sizes up rapidly if the market still looks attractive
- the paired sell-active mode commits the most gross capital
- the one-sided branch is smaller overall

Implementation implication:

- a single fixed `pair_budget_usd` does not match the observed strategy well
- the strategy should expose at least:
  - `probe_budget_usd`
  - `max_pair_budget_usd`
  - `max_single_side_budget_usd`
  - a short burst / layering window after the probe

### 4B. Capital Envelope

For the BTC 5m slice only, the minimum bankroll needed to reproduce the historical sizing was about `$6,696.74`, based on reconstructed cashflows from buys, sells, and settlement.

This does not mean the wallet only had that much capital. It means:

- the strategy is highly capital-efficient
- it reuses capital aggressively every `5m`
- a scaled-down copy should scale bankroll approximately linearly

Observed context:

- average daily buy notional: about `$376,997`
- median daily buy notional: about `$361,999`
- average daily turnover on rolling equity: about `5.16x`

Implementation implication:

- the runtime should think in terms of capital recycling and mode-specific budgets
- it should not assume that historical gross notional implies the same amount of static bankroll

### 4C. Small-Account Operating Profile

/todo for handoff agent:

- implement this as a dedicated config/risk mode for the initial live deployment
- do not infer that shrinking a single budget field is sufficient
- default the one-sided branch to disabled unless explicitly enabled

If the implementation is starting with a much smaller bankroll, it should scale the strategy down explicitly rather than trying to imitate the historical wallet's gross sizing.

Example small-account target:

- total bankroll: `$180`
- maximum capital deployed in any single market: `$60`
- practical goal: preserve enough idle capital to survive failed pair completion, cleanup, and the next market cycle

Important distinction:

- `max_gross_deployed_per_market` is not the same as "maximum amount willing to lose"
- the more relevant live limits are:
  - `max_gross_deployed_per_market`
  - `max_unpaired_exposure_usd`
  - `max_cleanup_loss_usd`

Recommended small-account profile for initial deployment:

- `max_gross_deployed_per_market`: `$40-$60`
- `probe_budget_usd`: `$1-$2`
- `initial_burst_budget_usd`: `$3-$5`
- `max_pair_budget_usd`: `$40-$50`
- `max_single_side_budget_usd`: `$8-$12`
- `max_unpaired_exposure_usd`: `$10-$15`
- `max_cleanup_loss_usd`: `$3-$5`

Recommended operating stance:

- start pair-first
- disable the one-sided branch entirely at first, or cap it very tightly
- if the second side cannot be acquired cheaply and quickly, stop escalating
- if cleanup mode is entered, stop adding risk immediately

Why this is necessary:

- the historical strategy depended on probe-and-burst scaling plus fast capital recycling
- with a small bankroll, order-size granularity and fees matter more
- a tiny account can tolerate gross deployment, but not much stranded one-sided exposure

Implementation implication:

- the handoff agent should treat the small-account profile as its own risk mode
- it should not just scale down `pair_budget_usd` and leave the rest of the logic unchanged
- one-sided continuation should be considered optional, not core, for a `$180` bankroll

### 4D. Performance Envelope

This edge is real, but it is not a huge per-trade edge. It compounds because the bot trades nearly every `5m` market and reuses capital aggressively.

Observed BTC 5m full-history performance:

- profitable markets: `4,148 / 7,083` or `58.6%`
- total PnL: `$318,944.05`
- full-history PnL over gross buy cost: about `3.02%`
- average daily PnL over daily buy cost: about `2.83%`
- median daily PnL over daily buy cost: about `2.85%`

Interpretation:

- the strategy does not need a huge edge per market
- it needs repeated access to early mispricing plus fast capital turnover
- the next implementation should optimize for consistency and inventory control, not for a heroic directional threshold

### 5. The One-Sided Branch Is Real But Small

- `192` markets, `2.7%` of all markets
- `116` winners, `76` losers
- Total PnL: `$14,791.99`
- Median entry offset: `9s`

This branch is not well explained by "the opposite side was unavailable."

Public market-trade proxy inside the first `15s` and `30s` after open shows:

- In one-sided winners, the median opposite-side minimum trade price in `15s` was `0.41`
- In one-sided losers, it was `0.48`
- In both winners and losers, the market-wide pair trade sum was below `$1` within `15s` almost always

Interpretation:

- The bot did not stay one-sided because pairing was impossible.
- The one-sided branch appears deliberate.

## Medium-Confidence Findings

### 6. Likely One-Sided Branch Logic

Best current read:

- The one-sided branch is a continuation bet, not a forced fallback.
- It is more likely when the initial move is aligned and reasonably strong, but the rule is not a single clean threshold.

Useful constraints from the data:

- `aligned entry move` means how much Binance had already moved in the bought direction by the time the bot entered
- `aligned close move` means how much the full 5-minute window ultimately finished in the bought direction
- One-sided winners had median aligned entry move of `+1.95 bps`
- One-sided losers had median aligned entry move of `+0.60 bps`
- One-sided winners had median aligned close move of `+12.34 bps`
- One-sided losers had median aligned close move of `-5.54 bps`

Interpretation:

- winners did not just have a slightly better entry snapshot
- they got real follow-through after entry
- losers often had a small favorable move at entry too, but it faded or reversed

Candidate sub-branch worth treating as plausible, not proven:

- if aligned entry move is above roughly `2 bps`
- and entry still happens by about `7s`
- then the bot sometimes accepts directional carry instead of finishing the pair

Support:

- One-sided markets with `aligned_entry_bps > 2` and `entry_offset <= 7s` had `79.4%` win rate and about `$10.0k` PnL across `34` markets

Why this is only medium-confidence:

- It explains only part of the one-sided sample.
- There is no single threshold that cleanly separates one-sided from paired markets.

### 7. Binance Matters, But As A Fast Reference Tape

Evidence does not support a hard BTC threshold like `0.12%` for BTC 5m.

What is supported:

- the wallet acts very early
- it usually aligns with the fast move rather than fighting it
- paired-with-sells is nearly direction-neutral by market close
- paired-no-sells and one-sided modes keep more directional carry

Interpretation:

- Binance is the fast clock
- Polymarket is the slower binary market
- the strategy is about pricing and timing first, direction second

### 7A. Binance WebSocket Requirements

The live implementation should use Binance as a first-class reference feed, not as an offline validation source only.

Required live input:

- `BTCUSDT` Binance trade or agg-trade websocket
- low-latency continuous last-trade stream
- per-market-open reference price captured at `t=0`

What the runtime should compute from Binance during the first `15-30s` after market open:

- signed move from market open
- absolute move from market open
- aligned move relative to the currently considered side
- very short continuation / reversal state over the last `1-3s`
- feed freshness

Recommended live construction:

- capture the first Binance trade at or immediately after the `5m` boundary as the opening reference
- keep a short rolling buffer of Binance trades for at least the last `3-5s`
- recompute move features on every new Binance trade, not on a slow timer
- refuse to treat Binance as valid if the feed is stale or the opening reference was never captured

What Binance should inform in the decision loop:

- which side looks stale relative to the fast reference move
- whether the bot should buy the first side at all
- whether a one-sided continuation branch is even eligible
- whether a second leg still makes sense if the reference move has reversed

Recommended usage:

- use Binance to bias the first side toward the fast move, not to hard-code a single global threshold
- use it as a veto when the fast move is stale, reversed, or unavailable
- use it as a stronger gating signal for the one-sided branch than for the paired branch
- a reasonable implementation split is:
  - paired branch can trigger on modest Binance confirmation if pair pricing is strong
  - one-sided branch should require both positive aligned move and short-horizon continuation

Practical live guidance:

- paired branch:
  - allow if Binance is fresh, entry is still early, and Polymarket pair pricing is attractive
  - use Binance mainly to pick which side to probe first and to veto clearly stale / reversed conditions
- one-sided branch:
  - require stronger Binance confirmation than the paired branch
  - use early aligned move plus recent continuation as the gating condition
  - if Binance reverses before the branch is fully established, prefer cleanup over conviction

Do not use Binance `1s` klines as the primary live decision feed. They are acceptable as a secondary monitoring / validation layer, but the live decision loop should be driven by the lower-latency trade stream.

### 7B. Gap Versus Current Runtime

The current `btc5m.rs` runtime is not yet a faithful implementation of this spec.

What it currently does well:

- schedule-aware market discovery
- entry window gating
- Polymarket book-based pair checks
- cleanup on partial-pair failure

What it is missing relative to the spec:

- no Binance websocket reference feed in the live decision loop
- no explicit probe-and-burst sizing model
- no explicit one-sided continuation branch
- no mode-specific budget logic
- no Binance-informed first-side selection or veto

Implementation implication:

- treat the current runtime as a partial pair-entry approximation
- do not treat it as spec-complete for live deployment
- /todo for handoff agent: close this gap before treating the strategy as implemented

### 7C. Live State And Gating Requirements

The next agent should implement this as a market-state machine, not as a single stateless trigger.

Minimum per-market live state:

- `open_ts`
- `binance_open_price`
- `latest_binance_price`
- `latest_binance_trade_ts`
- short rolling Binance trade buffer for `1-5s` continuation checks
- `first_leg_outcome`
- `first_leg_cost_usd`
- `second_leg_cost_usd`
- `mode_candidate` of `paired_with_sells`, `paired_no_sells`, or `one_sided`
- `spent_pair_budget_usd`
- `spent_single_side_budget_usd`
- cleanup / blocked flags

Hard vetoes the runtime should enforce:

- do nothing if Binance reference feed is stale or missing
- do nothing if entry has drifted outside the early window
- do nothing if available Polymarket size is too small for the intended clip
- do not escalate a first leg if Binance has already reversed against it
- do not keep adding exposure after cleanup mode has been entered

This matters because the historical strategy is not just "buy if cheap." It is "buy early, then adapt the rest of the sequence based on whether the fast tape still supports the trade."

### 7D. Order And Execution Semantics

The observed execution pattern is most consistent with aggressive, marketable limit orders rather than passive quoting.

Probable execution behavior:

- first leg is a small aggressive probe that takes displayed liquidity quickly
- second leg is larger and follows within seconds if the pair or continuation case still holds
- if the desired opposite-side fill is not available at an acceptable price, the bot does not keep blindly increasing the first leg
- if partial exposure remains after the edge degrades, the bot cleans up rather than averaging down

Implementation implication:

- the handoff agent should think in terms of `probe -> verify -> burst / pair / cleanup`
- a single static pair order is not a faithful representation of the observed wallet behavior

## Low-Confidence / Unknown

- Exact live quote thresholds because historical order book snapshots are not available
- Exact sell logic in the sell-active paired mode
- Exact branch rule for choosing paired carry versus one-sided continuation
- Exact probe-to-burst scaling schedule
- Whether the same spec generalizes to ETH, SOL, XRP, or BTC 15m without separate validation

## Probable Control Loop

This is the best handoff-safe pseudocode for BTC 5m:

```text
Every 5 minutes:
  detect new BTC 5m market open
  capture Binance BTC reference price at open
  stream Binance trade / agg-trade updates continuously

  around t = 5-9s:
    compute signed move, aligned move, short continuation, and feed freshness
    inspect Polymarket books
    fire a small probe on the cheaper / stale side, usually aligned with Binance

  during roughly the next 2-20s:
    if both sides can be acquired at an attractive effective sum:
      scale into the pair and complete the second side

      if the bot is in fast microstructure mode:
        recycle inventory with sells
      else:
        hold paired inventory into settlement

    else if Binance still shows strong aligned continuation and entry is still early:
      allow a smaller one-sided continuation position

    else:
      skip or clean up partial exposure
```

## Parameters To Hand Off

Use these as the current best ranges, not hard constants:

- Market cadence: `300s`
- First action: typically `5-9s` after open, broader high-confidence band `5-21s`
- Second-side completion: median `10s` overall, often `2-18s` depending on paired mode
- Probe sizing:
  - first fill row often around `$10-$20`
  - first `1s` burst more like `$40-$55`
- Mode-specific gross capital:
  - paired-with-sells median total buy cost about `$1.5k`
  - paired-no-sells median total buy cost about `$690`
  - one-sided median total buy cost about `$465`
- Small-account profile for `$180` bankroll:
  - target `max_gross_deployed_per_market` around `$40-$60`
  - target `probe_budget_usd` around `$1-$2`
  - target `max_pair_budget_usd` around `$40-$50`
  - target `max_single_side_budget_usd` around `$8-$12`, or disable one-sided entirely at first
  - target `max_unpaired_exposure_usd` around `$10-$15`
  - target `max_cleanup_loss_usd` around `$3-$5`
- Same-window pair target:
  - realistic achieved pair sums cluster around `0.95-0.98`
  - not the earlier hindsight-style `0.70` minima
- Binance reference input:
  - live `aggTrade` / trade stream
  - capture move from open, very short continuation / reversal state, and feed freshness
- One-sided candidate gate:
  - treat `aligned_entry_bps > 2` with still-early entry as a plausible branch input
  - do not treat it as a hard proven threshold
- One-sided branch: small, deliberate, and lower-confidence than the paired core
- Execution semantics:
  - first leg should be a small aggressive probe
  - second leg should be conditional on refreshed Binance and acceptable pair / continuation state
  - cleanup logic is part of the core strategy, not a bolt-on safety afterthought

## Confidence Summary

High confidence:

- schedule-driven BTC 5m market-open bot
- paired inventory is the dominant strategy
- contemporaneous pair capture is real
- two distinct paired modes exist

Medium confidence:

- one-sided continuation branch is deliberate
- one-sided branch is more likely when aligned early move is stronger and still early

Low confidence:

- exact live quote thresholds
- exact sell rules
- exact one-sided branch threshold

## Supporting Files

Read first:

- `tmp/cplus/btc_5m_strategy_spec.md`
- `tmp/cplus/report.md`
- `tmp/cplus/market_summary.csv`
- `tmp/scripts/analyze_btc_5m_cplus.py`

Optional deep dives:

- `tmp/cplus/pair_capture_summary.csv`
- `tmp/cplus/weekly_summary.csv`
- `tmp/cplus/one_side_summary.csv`
- `tmp/cplus/single_side_enriched.csv`
- `tmp/cplus/history_profile_summary.csv`
- `tmp/cplus/march_profile_summary.csv`
- `tmp/scripts/analyze_btc_5m_attribution.py`
- `tmp/supplemental_march_attribution/report.md`
- `tmp/supplemental_march_attribution/market_summary.csv`
- `tmp/supplemental_march_attribution/single_side_markets.csv`
- `tmp/supplemental_march_attribution/single_side_by_profit.csv`
