# Research: Master Strategy — Universal Pricing, Execution & Risk Framework

## Overview

Several tweets reference a pair of whitepaper-style formula pages that underpin a general-purpose Polymarket trading system. Combined with insights from a detailed hedge fund breakdown and multiple calibration/risk management threads, these form a "master strategy" — a framework of pricing, execution, and risk management primitives that can be applied to **any** Polymarket market type.

This document synthesizes the universal components. The market-specific research docs (BTC up/down, weather, market maker rewards) describe how to specialize these primitives for particular market categories.

## Sources

| Author | Handle | Key Contribution |
|---|---|---|
| Mr. Buzzoni | @polydao | Decoded the 2-page whitepaper: LMSR pricing, EV formula, Bayesian updating |
| may.crypto | @xmayeth | Original whitepaper source; RL agent trained on Polymarket data |
| self.dll | @seelffff | Black-Scholes for binary options; adverse selection alpha formula |
| ramper | @ramperxx | Brier score calibration framework |
| Discover | @0x_discover | Brier score monitoring; market selection |
| Archive | @archiveexplorer | Rust + Python hybrid architecture for sports betting bots |
| Roan | @rohonchain | Full prediction market hedge fund breakdown |

### Source Links (actually accessed)

- https://api.fxtwitter.com/polydao/status/2030029152997245077 — decoded the 2-page whitepaper: LMSR, EV formula, Bayesian updating
- https://api.fxtwitter.com/xmayeth/status/2030306457925636125 — original whitepaper source, RL agent
- https://api.fxtwitter.com/seelffff/status/2030351248608702593 — Black-Scholes for binary options, $800→$400K claim
- https://api.fxtwitter.com/seelffff/status/2030310382020001936 — adverse selection alpha formula
- https://api.vxtwitter.com/ramperxx/status/2029667340309209538 — Brier score calibration
- ~~https://x.com/0x_discover/status/2030200246534365459~~ — **NOT independently fetched**; content inferred from @ramperxx tweet (near-identical thread). Treat as single-source for Brier score material.
- https://api.fxtwitter.com/archiveexplorer/status/2028903434791854194 — Rust+Python sports bot architecture, $2K→$10.8K weekly
- https://api.fxtwitter.com/rohonchain/status/2029998336837890193 — prediction market hedge fund breakdown, 5 strategies, Rust production stack
- https://pbs.twimg.com/media/HCy3by7WsAAa638.jpg — Brier Score Calibration Heatmap image (shared by both @ramperxx and @0x_discover)

## Part 1: The Whitepaper Formulas

@xmayeth shared two pages of formulas from "a friend at a quant fund" claiming $400K/year on Polymarket. @polydao decoded them:

### Page 1 — How Polymarket Prices Outcomes (LMSR)

Polymarket uses the **Logarithmic Market Scoring Rule**:

```
C(q) = b * ln(Σ e^(qi/b))
```

The probability displayed in the market is a softmax:

```
pi = e^(qi/b) / Σ e^(qj/b)
```

This is literally the same math as neural network output layers. The market itself acts as a classifier that aggregates beliefs. Understanding this is foundational — it tells you how the market prices should behave and where mechanical mispricings can occur.

### Page 2 — How the Bot Decides When to Act

The core expected value formula:

```
EV = p_hat * (1 - p) - (1 - p_hat) * p = p_hat - p
```

Where:
- `p_hat` = your estimated true probability
- `p` = current market probability

**If `p_hat > p`, you have positive edge. Trade.**

The system continuously updates beliefs using Bayesian inference in log-space:

```
log P(H|D) = log P(H) + Σ log P(Dk|H) - log Z
```

The full loop:
1. Estimate the real probability (`p_hat`) from external data
2. Compare to market price (`p`)
3. If EV > 0, place trade
4. When new data arrives, update `p_hat` via Bayesian update
5. Repeat

### Execution Speed Matters

From @polydao's analysis of the whitepaper's latency table: the real edge isn't the formulas — it's completing the full cycle (data → update → trade) in **~828ms**. The formulas are table stakes; speed is the moat.

## Part 2: Theoretical Fair Value Pricing

### Black-Scholes for Binary Options (from @seelffff)

Every Polymarket contract is a binary option. Binary option pricing via Black-Scholes:

```
V = e^(-rT) * N(d2)
d2 = (ln(S/K) + (r - sigma^2/2) * T) / (sigma * sqrt(T))
```

Where:
- `V` = theoretical fair price of the binary contract
- `r` = risk-free rate (can use 0 for short-duration markets)
- `T` = time to expiry
- `S` = current price of the underlying
- `K` = strike price (the threshold defining the binary outcome)
- `sigma` = implied volatility
- `N()` = cumulative normal distribution function

@seelffff claims $800 → $400,000 over 6 months using this systematically — finding markets where the theoretical price diverges from the market price.

**Example**: Market trading at 0.44, Black-Scholes says 0.61 (39% mispriced). Enter $800, resolved at 0.98, profit +$31,400.

### When to Use Which Model

| Market Type | Best Pricing Model | Why |
|---|---|---|
| Crypto up/down (5m/15m) | GBM + EWMA/GARCH vol | Price follows continuous stochastic process |
| Crypto up/down (1H+) | Black-Scholes binary | Longer duration, vol estimation more stable |
| Weather | Bayesian from forecast models | Probability comes from meteorological data, not price dynamics |
| Sports | Sport-specific ML models | Each sport has different dynamics (see @archiveexplorer) |
| Political/event | Bayesian + news sentiment | Requires qualitative signal processing |

## Part 3: Position Sizing — Kelly Criterion

Every source converges on Kelly:

```
f* = (p * b - q) / b
```

Where:
- `f*` = fraction of bankroll to bet
- `p` = estimated true probability
- `b` = net payout odds (profit per $1 wagered)
- `q` = 1 - p

**Important adjustments:**
- Use **fractional Kelly** (e.g., half-Kelly or quarter-Kelly) in practice — full Kelly assumes perfect probability estimates, which you never have
- @rohonchain's hedge fund article notes that "empirical adjustments" to Kelly are standard in production
- Cap maximum position size (e.g., 2-5% of bankroll) regardless of what Kelly says

## Part 4: Risk Management

### Brier Score Calibration (from @ramperxx and @0x_discover)

```
Brier = Σ (p_predicted - outcome)^2
```

Where 0.00 = perfect calibration, 0.25 = random guessing.

**Operational rules:**
- Check Brier score every **~50 trades**
- If Brier rises from ~0.12 to ~0.19, **stop trading** — your edge has disappeared
- Winning with bad calibration = luck. Losing with good calibration = variance. Only Brier score distinguishes them.
- Build an automated circuit breaker: halt trading when Brier crosses threshold

### Adverse Selection Filter (from @seelffff)

```
alpha = Delta / (V_h - mu)
```

Where `alpha` = fraction of informed traders in a market.

**Rule: Do not enter markets where `alpha > 12%`.**

Wide spreads mean smart money is present — you become someone else's profit if you enter.

### VPIN — Volume-Synchronized Probability of Informed Trading (from @rohonchain)

The hedge fund breakdown describes using VPIN as a risk trigger:
- Monitor VPIN continuously per market
- When VPIN > 0.6, **exit or reduce exposure** — informed trading volume is too high

### Portfolio-Level Risk

From @rohonchain's hedge fund model:
- VaR (Value at Risk) calculations for portfolio-level monitoring
- 5 desk roles: Research, Execution, Risk, Strategy, DevOps
- Risk desk monitors aggregate exposure, correlation between positions, and VPIN across all active markets

## Part 5: Architecture Reference

### Hedge Fund Stack (from @rohonchain)

| Component | Technology |
|---|---|
| Prototyping | Python / NumPy |
| Production execution | **Rust** |
| Time-series DB | TimescaleDB |
| Graph DB | Neo4j |
| Cache | Redis |
| Message queue | Apache Kafka |
| Data feeds | Bloomberg, The Odds API, FiveThirtyEight API |

### Sports Bot Architecture (from @archiveexplorer)

| Component | Technology |
|---|---|
| Data ingestion | Rust — WebSocket from Sportradar, protobuf/JSON parsing, ZeroMQ |
| ML inference | Python — LSTM, LightGBM, Gradient Boosting, Monte Carlo |
| Model runtime | ONNX (3-5x faster than sklearn) |
| Execution | Rust — EIP-712 signing, Polymarket CLOB, Kelly sizing, stop-loss |
| End-to-end latency | <50ms |

Both architectures validate RTT's approach: **Rust for the hot path (data ingestion + execution), with a separate layer for intelligence/ML**.

### The Five Trading Strategies (from @rohonchain's hedge fund)

1. **Conditional arbitrage** — detecting logical constraint violations across related markets (e.g., P(A and B) can't exceed P(A))
2. **Calibration surface shorts** — exploiting longshot bias (markets <15% are systematically overpriced)
3. **Sportsbook lag capture** — exploiting 1-2 minute pricing delays vs. sports data feeds
4. **Resolution front-running** — mempool monitoring for early resolution signals
5. **Cross-venue arbitrage** — price discrepancies across Polymarket, Opinion, Betfair

## Risks

These are risks to the overall framework — assumptions that could be wrong, sources that could be unreliable, and models that could be misapplied.

1. **Market efficiency assumption.** The entire framework assumes you can estimate `p_hat` better than the market. If the market is efficient (even partially), your `p_hat` estimates are just noise around the true price, and trading on them generates negative EV after fees. The market efficiency assumption needs to be tested per market category, not assumed away.

2. **Bayesian likelihood misspecification.** Bayesian updating in log-space sounds elegant but requires specifying likelihood functions `P(Dk|H)` for each data source. Getting these wrong means the posterior drifts in the wrong direction. Garbage-in-garbage-out applies — bad likelihoods are worse than no updating at all.

3. **Correlated Kelly overexposure.** Kelly criterion assumes known probabilities and independent bets. On Polymarket, bets are correlated (multiple crypto markets move together, related political markets resolve in clusters). Naive Kelly across correlated positions leads to overexposure.

4. **LMSR vs. CLOB mismatch.** The LMSR formulation in the whitepaper may not reflect how Polymarket actually works today. Polymarket uses a CLOB (Central Limit Order Book), not a pure AMM. The LMSR may apply to initial pricing or the AMM component, but most volume flows through the order book where prices are set by competing limit orders, not by a cost function.

5. **Unverified hedge fund source.** The "hedge fund breakdown" from @rohonchain is an X article — not a published paper, not audited, and the author's identity/credentials are unverified. It reads plausibly but could be aspirational or fictional. Don't treat it as ground truth.

6. **Black-Scholes assumptions don't hold.** The Black-Scholes binary pricing model assumes log-normal returns and continuous trading — neither holds perfectly on Polymarket. For short-duration markets (5m, 15m), the distribution is far from log-normal. For illiquid markets, continuous trading doesn't apply.

7. **Extraordinary return claims.** The $800→$400K claim from @seelffff is extraordinary and unverified. If true, it would represent one of the highest documented returns from systematic prediction market trading. Treat with extreme skepticism — selection bias (only winners tweet) and survivorship bias are rampant in crypto trading Twitter.

8. **Arbitrary Brier thresholds.** The Brier score thresholds (0.12 good, 0.19 bad) are arbitrary numbers from a Twitter thread. The appropriate Brier thresholds depend on the market category, the base rate of outcomes, and the bot's strategy. These need to be calibrated empirically, not taken as gospel.

9. **Self-reported architectures.** All architecture references (Rust, TimescaleDB, Kafka, etc.) are from self-reported setups on Twitter. No one has verified that these systems actually exist as described.

## Part 6: High-Level Approach for RTT

### Universal Decision Pipeline

Every strategy, regardless of market type, should flow through the same pipeline:

```
1. DATA INGEST     →  Market-specific data source (Binance, NOAA, etc.)
2. FAIR VALUE      →  Compute p_hat using appropriate model
3. EDGE DETECTION  →  edge = p_hat - p_market
4. MARKET FILTER   →  alpha < 0.12 AND VPIN < 0.6
5. POSITION SIZE   →  Kelly criterion (fractional)
6. EV GATE         →  EV > threshold
7. EXECUTE         →  CLOB order via Rust execution layer
8. MONITOR         →  Brier score every 50 trades, halt if degrading
```

Steps 1-2 are market-specific. Steps 3-8 are universal.

### What RTT Should Build as Shared Infrastructure

1. **Edge detection module**: Takes `p_hat` and `p_market`, returns edge and trade direction
2. **Kelly sizer**: Takes edge and bankroll, returns position size (with configurable fraction)
3. **Market screener**: Computes `alpha` and VPIN for all markets, maintains allow/deny list
4. **Calibration tracker**: Tracks Brier score per strategy, triggers halt when degrading
5. **Execution engine**: Already exists in RTT — CLOB order management, EIP-712 signing
6. **P&L tracker**: Per-strategy and aggregate, with daily/weekly reporting

### Strategy-Specific Modules (Plug Into Universal Pipeline)

| Strategy | Data Source | Fair Value Model | Entry Threshold |
|---|---|---|---|
| BTC up/down | Binance WebSocket | GBM + EWMA/GARCH | edge > 0.06 |
| Weather | NOAA / ECMWF / GFS | Bayesian from forecast CI | gap > 0.15 |
| Market maker rewards | Polymarket CLOB only | N/A (spread capture) | alpha < 0.12, reward pool active |
| General binary | Market-dependent | Black-Scholes binary | edge > 0.10 (conservative) |

### Implementation Priority

1. **Market maker rewards** — simplest, no ML, immediate revenue, validates execution layer
2. **BTC up/down** — highest volume, clearest edge, leverages existing RTT infrastructure
3. **Weather** — uncorrelated returns, data edge is durable, moderate complexity
4. **General binary (Black-Scholes)** — requires the most judgment on which markets to enter, save for later

## Sanity Check

Before building on top of this framework, verify the foundational assumptions. Each check is designed to catch bad assumptions early, before they compound into a system that trades confidently on wrong beliefs.

1. **LMSR verification.** Check whether Polymarket's CLOB actually uses LMSR for pricing, or if LMSR is only the AMM component. The CLOB may use a standard price-time priority matching engine with no LMSR involvement. This changes whether the softmax probability analysis is relevant to live trading or just to understanding the AMM's initial seeding.

2. **EV formula verification.** The simplified `EV = p_hat - p` assumes unit-size bets. For Kelly-sized bets, the EV calculation is more complex. Verify that the simplified formula is valid for the position-sizing approach used.

3. **Bayesian updating verification.** Implement the log-space Bayesian update and test it against a simple moving-average baseline. If the Bayesian update doesn't outperform a moving average of recent price data, the added complexity isn't justified.

4. **Black-Scholes verification.** Compute Black-Scholes fair values for a sample of 100+ resolved Polymarket contracts. Compare against actual resolution prices. If the model doesn't predict better than the market's last traded price, it's not adding value.

5. **Brier score verification.** Compute Brier scores for the bot's predictions across a statistically meaningful sample (200+ trades minimum). The 50-trade checkpoint is for early warning, but draw conclusions only from larger samples.

6. **Alpha formula verification.** The formula `alpha = Delta / (V_h - mu)` originates from a single Twitter thread. This resembles the PIN (Probability of Informed Trading) framework from academic market microstructure literature (Easley & O'Hara). Verify the formula against the original academic work to ensure it's correctly specified, and test empirically on Polymarket order flow data.

7. **VPIN verification.** VPIN at 0.6 as a threshold comes from the @rohonchain article. Academic VPIN literature (Easley, Lopez de Prado, O'Hara) typically uses different calibrations. Verify the 0.6 threshold by backtesting against historical Polymarket data — does VPIN > 0.6 actually predict adverse selection events?

8. **Kelly fraction verification.** Paper-trade with full Kelly, half-Kelly, and quarter-Kelly over the same trade sequence. Compare terminal wealth and maximum drawdown. The right fraction depends on estimation error in `p_hat`, which varies by strategy.

9. **Architecture claims verification.** The hedge fund stack (TimescaleDB, Neo4j, Kafka, Rust) and the sports bot stack (Sportradar, ZeroMQ, ONNX) are self-reported. Before adopting any of these technology choices, evaluate them on their own merits for RTT's specific needs, not because someone on Twitter said they use them.
