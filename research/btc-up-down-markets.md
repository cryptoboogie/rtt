# Research: BTC Up/Down Markets Strategy

## Overview

Polymarket's 5-minute and 15-minute BTC (and other crypto) "Up or Down" markets are binary contracts that resolve based on whether the price of BTC/SOL/XRP moves up or down within a fixed window. These markets are the highest-volume, most liquid prediction markets and present a clear latency arbitrage opportunity.

## Sources

| Author | Handle | Key Contribution |
|---|---|---|
| Lorden | @lorden_eth | BTC latency arb architecture, formulas, exploitable window |
| Tengen | @0xTengen_ | Real wallet ($240K profit), fee adaptation, deep limit orders |
| PolyFair | @polyfair_ | Three GBM-based pricing models for fair value estimation |
| self.dll | @seelffff | Black-Scholes binary option pricing for theoretical fair value |

## Source Links (actually accessed)

- https://api.fxtwitter.com/lorden_eth/status/2030316141889904724 — BTC latency arb, formulas, exploitable window
- https://cdn.syndication.twimg.com/tweet-result?id=2029211154539790845&lang=en&token=x — @0xTengen_, wallet "vague-sourdough", $240K profit, fee adaptation (truncated; supplemented by quote-tweet at id=2025913717330997596)
- https://api.fxtwitter.com/polyfair_/status/2030277962424025499 — PolyFair tool, GBM pricing models
- https://api.fxtwitter.com/seelffff/status/2030351248608702593 — Black-Scholes binary option pricing
- https://api.fxtwitter.com/seelffff/status/2030310382020001936 — adverse selection alpha formula
- https://api.fxtwitter.com/polydao/status/2030029152997245077 — LMSR/whitepaper decode (informed the LMSR section)
- https://api.fxtwitter.com/rohonchain/status/2029998336837890193 — hedge fund breakdown (informed Kelly/risk sections)

## How Polymarket Prices These Markets (LMSR / Softmax)

Polymarket uses the **Logarithmic Market Scoring Rule (LMSR)** to price all contracts, including crypto up/down:

```
C(q) = b * ln(Σ e^(qi/b))
pi = e^(qi/b) / Σ e^(qj/b)     ← softmax (same math as neural net output layers)
```

The displayed probability `pi` is a softmax over outstanding share quantities. This matters for BTC up/down because:

1. **Mechanical lag**: The LMSR reprices based on *on-chain order flow*, not external price feeds. When Binance moves, the softmax output stays stale until someone trades on Polymarket — that's the exploitable window.
2. **Predictable repricing**: Because LMSR is a continuous, deterministic function, you can predict exactly what the market price *should* be after a Binance move, not just the direction. This lets you compute precise edge rather than guessing.
3. **Liquidity parameter `b`**: The `b` parameter controls how sensitive prices are to trades. Low-`b` markets (less liquid) reprice more violently per trade — both opportunity and risk.

## The Opportunity

Polymarket's crypto "Up or Down" markets reprice slower than centralized exchanges. When Binance moves, there is a **7-17ms exploitable window** before Polymarket reprices:

```
t=0ms      Real price moves (Binance)
t=15-23ms  Bot reacts
t=30-40ms  Polymarket reprices
```

This is a textbook latency arbitrage: use a faster data source (Binance WebSocket) as a "true price" oracle, compare against Polymarket's stale odds, and enter before repricing.

### Proven Results

- @lorden_eth claims $2-4K/month from this strategy
- @0xTengen_ tracked wallet "vague-sourdough": **$240K profit, 14,767 predictions**, survived Polymarket's 3.15% taker fee increase
- @polyfair_ built a live analytics tool (polyfair.pro) with three pricing models specifically for crypto markets

## Core Formulas

### 1. Edge Detection
```
edge = p_true - p_market
```
Where `p_true` is derived from Binance price data and `p_market` is the current Polymarket price. Enter only when `edge > 0.06` (6 cents).

### 2. Kelly Criterion for Position Sizing
```
f* = (p * b - q) / b
```
Where `p` = true probability, `q` = 1 - p, `b` = payout odds. This tells you exactly what fraction of bankroll to risk.

**Use fractional Kelly in practice.** Full Kelly assumes your `p_true` estimate is perfect — it never is, even with a good GBM model. Standard practice (confirmed by @rohonchain's hedge fund breakdown) is to use **half-Kelly or quarter-Kelly**. This reduces variance dramatically at the cost of a small reduction in expected growth rate. For 5-minute BTC markets where you're placing many trades per day, quarter-Kelly is appropriate — you get enough repetitions that the reduced sizing is more than compensated by survival.

### 3. Bayesian Updating of p_hat

The `p_true` estimate should not be static. As new Binance ticks arrive within a market's window, update beliefs continuously using Bayesian inference in log-space:

```
log P(H|D) = log P(H) + Σ log P(Dk|H) - log Z
```

The loop: compute initial `p_true` from GBM model → observe new price ticks → Bayesian update `p_true` → re-evaluate edge. This is especially important for 5-minute markets where the underlying can move significantly between your initial estimate and order fill. A stale `p_true` is as dangerous as a stale `p_market`.

### 4. Expected Value Filter
```
EV = (p * profit) - (q * loss)
```
Only trade when EV is positive. This is the final gate before execution.

### 5. Black-Scholes for Binary Options (Fair Value)
```
V = e^(-rT) * N(d2)
d2 = (ln(S/K) + (r - sigma^2/2) * T) / (sigma * sqrt(T))
```
Every Polymarket binary contract has a theoretical Black-Scholes price. When market price diverges significantly from theoretical price, there is a trade.

### 6. GBM Fair Price Models (from PolyFair)

Three models for estimating the "true" probability of crypto price movement:

- **LN_EWMA**: Lognormal GBM with Exponentially Weighted Moving Average volatility
- **LN_GARCH**: Lognormal GBM with GARCH(1,1) volatility forecasting
- **T_EWMA**: Student-t distribution (fat tails) with EWMA volatility

These compute fair prices from historical price data and compare against CLOB prices.

## Fee Regime

Polymarket introduced a **3.15% dynamic taker fee** on short-duration crypto markets. The surviving bots adapted by:

1. **Widening required spread** beyond the fee to maintain profitability
2. **Using limit orders (maker)** instead of taker orders to avoid the fee entirely and earn rebates
3. **Deep limit order placement** to catch extreme slippage during panic selling (tail-risk harvesting)

The wallet tracked by @0xTengen_ showed ROIs of 4,000-7,200% on tail events (buying SOL at 1.3 cents, XRP at 2.4 cents during panic).

## Risks

**Latency arms race.** The 7-17ms exploitable window will shrink as more bots compete. This is a red queen problem — the edge degrades as competition increases. If the window compresses to 2ms, the strategy may require co-location or custom network infrastructure to remain viable.

**Fee regime changes.** Polymarket already raised taker fees to 3.15%. Further increases could make marginal trades unprofitable. The strategy must be re-evaluated after every fee change — a 5% taker fee, for example, would kill most latency arb edge on near-50/50 contracts.

**Binance data reliability.** The entire strategy depends on a single oracle: Binance spot price. If Binance has a flash crash, API glitch, or WebSocket outage, the bot trades on garbage data. Circuit breakers on the data feed itself are mandatory — not just on P&L. Stale data detection (no tick in N ms), price spike detection (move > X sigma in one tick), and feed failover should all be built before going live.

**Regulatory risk.** Polymarket's legal status is uncertain in some jurisdictions. The bot could be profitable but inaccessible if regulatory action restricts access or forces withdrawal of funds.

**Model overfitting.** GBM/GARCH parameters tuned on historical data may not generalize. Crypto volatility regimes change — bull vs. bear markets shift vol structure dramatically. A model calibrated during a low-vol regime will systematically misprice during a vol expansion (and vice versa). Walk-forward validation is essential.

**Counterparty risk.** Polymarket is a centralized platform. Funds on the platform are at risk of platform failure, hacks, or withdrawal restrictions. Do not keep more capital on-platform than is needed for active trading.

**Unaudited source claims.** The "vague-sourdough" wallet and claimed $240K returns are unaudited — we only see Polymarket profile screenshots shared on Twitter. The strategy may be real but the magnitude of returns could be exaggerated or cherry-picked. Treat all third-party performance claims as directional signal, not ground truth.

## High-Level Approach for RTT

### Data Pipeline
1. **Binance WebSocket** for real-time BTC/SOL/XRP price data (sub-millisecond)
2. **Polymarket CLOB** for current market prices and order book state
3. Compute `p_true` from Binance data using one or more of the GBM models (LN_EWMA is simplest to start)
4. Compare against `p_market` from Polymarket

### Decision Engine (Universal Pipeline Specialization)

This plugs into the universal decision pipeline from the master strategy. Steps 1-2 are BTC-specific; steps 3-8 are shared infrastructure:

```
1. DATA INGEST     →  Binance WebSocket (BTC/SOL/XRP spot price)
2. FAIR VALUE      →  p_true via LN_EWMA / LN_GARCH / T_EWMA, with Bayesian updating
3. EDGE DETECTION  →  edge = p_true - p_market
4. MARKET FILTER   →  alpha < 0.12 AND VPIN < 0.6
5. POSITION SIZE   →  Quarter-Kelly, capped at 5% bankroll
6. EV GATE         →  EV > 0 (after fees)
7. EXECUTE         →  CLOB order via Rust execution layer
8. MONITOR         →  Brier score every 50 trades, halt if degrading
```

Entry threshold: `edge > 0.06` (6 cents) — tune empirically after live data.

### Order Strategy — Two Modes
1. **Latency arb mode**: Market/aggressive limit orders when edge is large and time-sensitive (accept taker fee)
2. **Passive mode**: Deep limit orders placed ahead of anticipated moves to catch tail events (earn maker rebates)

### Risk Controls

**Position sizing:**
- Fractional Kelly (half or quarter) capped at 5% of bankroll per trade

**Brier score calibration monitoring:**
```
Brier = Σ (p_predicted - outcome)^2
```
Where 0.00 = perfect, 0.25 = random guessing. Check Brier score every **~50 trades**. If it degrades from ~0.12 to ~0.19, **halt trading** — the edge has disappeared or market microstructure has changed. This is the only reliable way to distinguish genuine edge from luck. Build this as an automated circuit breaker.

**VPIN (Volume-Synchronized Probability of Informed Trading):**
Monitor VPIN continuously per market. When **VPIN > 0.6**, exit or reduce exposure — informed trading volume is too high. In BTC up/down markets, VPIN spikes often precede large directional moves driven by insiders or whales who have advance knowledge of exchange movements. Trading against informed flow is a guaranteed way to destroy edge.

**Adverse selection filter:**
Avoid markets where spread suggests informed trading (`alpha > 12%`). Wide spreads in BTC up/down markets mean smart money has already positioned — you become their exit liquidity.

**Daily P&L stop-loss:**
Hard stop on daily losses to survive variance.

### Priority for Implementation
This is the **most directly actionable** strategy for RTT because:
- RTT's Rust execution layer already handles EIP-712 signing and CLOB interaction
- Binance WebSocket ingestion is straightforward
- The math is simple (edge detection + Kelly)
- 5-minute markets provide rapid feedback for tuning
- Proven profitability even after fee increases

## Sanity Check

Items to verify before trusting this document's conclusions and before writing implementation specs:

1. **Verify the 7-17ms exploitable window.** Measure actual latency between Binance WebSocket and Polymarket CLOB in a test environment. If the window is actually <5ms, the strategy may not be viable without co-location. Set up a simple ping test: subscribe to Binance BTC/USDT, simultaneously poll Polymarket BTC 5-min market, measure the lag between a Binance move and the corresponding Polymarket reprice.

2. **Verify the 3.15% taker fee.** Check Polymarket's current fee schedule — fees change. Reference: https://docs.polymarket.com. If fees have increased since this document was written, re-run all EV calculations with the updated fee.

3. **Verify maker fee structure.** Confirm that maker orders are still fee-free or earn rebates. This is critical to the passive mode strategy. If Polymarket introduces maker fees, the passive/deep-limit-order approach loses its primary advantage.

4. **Cross-check the edge > 0.06 threshold against fees.** After a 3.15% taker fee, a $0.50 contract costs ~$0.016 in fees. Is 6 cents of edge enough to cover fees + slippage + the risk of adverse selection? Work through a concrete example: buy YES at $0.50, pay $0.016 fee, sell at $0.56 — net profit is $0.06 - $0.016 = $0.044 before slippage. If slippage eats 2 cents, net is $0.024. That may still be positive EV but the margin is thin.

5. **Test Black-Scholes against historical resolutions.** Does Black-Scholes actually produce useful fair values for 5-minute BTC binary markets? The model assumes log-normal returns, which may not hold for 5-minute crypto where jumps and microstructure noise dominate. Back-test against historical 5-minute market resolutions and compare BS predictions vs. actual outcomes.

6. **Verify Kelly criterion math via paper trading.** Record edge estimates over 1000+ simulated or paper trades. Compare Kelly-sized positions vs. fixed-size positions. If Kelly doesn't outperform fixed sizing on real data, the edge estimates may be too noisy for Kelly to add value.

7. **Independently verify GBM models.** The LN_EWMA, LN_GARCH, and T_EWMA models are from PolyFair's tool. Implement these independently and compare outputs against polyfair.pro. If outputs diverge significantly, determine whether the divergence is a bug in our implementation or an undocumented adjustment in PolyFair's.

8. **Check Polymarket's LMSR usage.** Verify whether Polymarket's CLOB actually uses the standard LMSR formulation or a variant. The CLOB may not use LMSR at all for the actual matching engine — LMSR may only apply to the AMM component. If the CLOB is a standard limit order book, the LMSR math in this document is informative background but not directly actionable for order placement.
