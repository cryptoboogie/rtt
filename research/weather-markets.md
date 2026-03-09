# Research: Weather Markets Strategy

## Overview

Polymarket hosts prediction markets on weather outcomes — temperature ranges, snowfall, frost events, heat indices, cyclone paths, and rainfall. These markets are structurally inefficient because most participants lack access to (or understanding of) raw meteorological data. The edge comes from ingesting professional forecast models and comparing their confidence intervals against market prices.

## Sources

| Author | Handle | Key Contribution |
|---|---|---|
| Hrundel75 | @hrundel75 | Simple Bayesian NOAA-based strategy, 117x return |
| Hanako | @hanakoxbt | Multi-model ensemble (6 sources), 142ms cross-check, Sharpe 12.96 |
| may.crypto | @xmayeth | RL agent originated from a weather market loss; general approach |

## Source Links (actually accessed)

- https://api.fxtwitter.com/hrundel75/status/2029331748467949883 — Bayesian NOAA strategy, $204→$24K wallet, 117x return
- https://api.fxtwitter.com/hanakoxbt/status/2029571502190825930 — multi-model ensemble bot, 6 weather models, Sharpe 12.96
- https://api.fxtwitter.com/xmayeth/status/2030306457925636125 — RL agent born from weather market loss, general approach
- https://api.fxtwitter.com/rohonchain/status/2029998336837890193 — hedge fund breakdown, informed risk management sections
- https://api.fxtwitter.com/polydao/status/2030029152997245077 — LMSR/whitepaper decode, informed LMSR context
- https://api.vxtwitter.com/ramperxx/status/2029667340309209538 — Brier score calibration framework

## The Opportunity

Weather markets are mispriced because:

1. **Information asymmetry**: Most Polymarket participants use consumer weather apps (Weather.com, Apple Weather). Professional forecast models (ECMWF, GFS, HRRR) update more frequently and provide probability distributions, not point estimates.
2. **Low competition**: These markets are "boring" compared to crypto or politics — fewer bots, wider spreads, bigger reward pools.
3. **Deterministic resolution**: Weather outcomes are objectively measurable (temperature at a station, snowfall in inches). No subjectivity in resolution.
4. **Frequent market creation**: New weather markets are created regularly for different cities and timeframes.
5. **Low adverse selection alpha**: Weather markets generally have few informed traders — most participants are retail. The adverse selection fraction `alpha = Delta / (V_h - mu)` tends to sit well below the 12% danger threshold. This is a structural reason weather markets are attractive: you're rarely trading against someone with a genuine information edge. The exception is extreme weather events (hurricanes, heat domes, polar vortex outbreaks), when specialized traders — meteorologists, weather derivatives desks, reinsurance analysts — enter the market and alpha can spike above 12%. Monitor alpha around extreme events and pull back when it rises.

### Proven Results
- @hrundel75 tracked a wallet: **$204 → $24,000 (117x), 1,516 trades, ~70% win rate**
- @hanakoxbt: **$9,685 overnight, 78 trades, 73.1% win rate, Sharpe 12.96** from multi-model arbitrage

## Strategy Variants

### Variant A: Simple Bayesian (from @hrundel75)

The simplest version. One data source, one formula:

1. **Read NOAA forecast grid** — raw meteorological data with confidence intervals
2. **Compare forecast confidence intervals to Polymarket bucket prices**
3. **If gap > 15%, enter position**

Core math: Bayes theorem, conditional probability, expected value. Nothing more.

```
P(outcome | forecast) vs P_market(outcome)
If gap > 0.15 → trade
```

This is a "level 1 probability" strategy — no stochastic calculus, no Black-Scholes, no neural networks. Just reading data that others aren't reading.

**LMSR context**: The market price you're comparing against isn't arbitrary — it's a softmax probability from Polymarket's LMSR cost function (`pi = e^(qi/b) / Σ e^(qj/b)`). This is the same math as a neural network output layer: the market price is literally a weighted aggregation of all participants' beliefs, shaped by how much capital backs each position. When your NOAA forecast disagrees with this aggregated belief, you're trading against the crowd's weighted average — and in weather markets, that crowd is mostly retail users checking Apple Weather, not meteorologists reading the raw model output.

### Variant B: Multi-Model Ensemble (from @hanakoxbt)

A more sophisticated version that ingests 6 forecast models and cross-references them:

**Data Sources (6 models):**
| Model | Provider | Update Freq | Strengths |
|---|---|---|---|
| ECMWF | European Centre | 2x daily | Gold standard for medium-range (3-15 days) |
| GFS | NOAA (US) | 4x daily | Good global coverage, free |
| HRRR | NOAA (US) | Hourly | High-res, best for short-range US weather |
| NAM | NOAA (US) | 4x daily | North American mesoscale, good for severe weather |
| UKMO | UK Met Office | 2x daily | Strong for Atlantic/European weather |
| CMC | Canada | 2x daily | Good for Arctic/Northern weather |

**Process:**
1. Ingest all 6 models every update cycle
2. Cross-check forecasts in **142ms**
3. Detect when model consensus disagrees with Polymarket pricing
4. Fire position before odds reprice
5. Exit on convergence (when market adjusts to match forecast)

**Works across all weather contract types**: temperature, snowfall, cyclone, frost, heat index, rain.

The key insight: **"It doesn't predict weather. It predicts where the market is wrong."** The bot is not a weather forecaster — it's an arbitrageur between professional models and a slow market.

**Bayesian updating loop**: Rather than replacing `p_hat` wholesale each time a forecast model updates, use Bayesian inference in log-space to incorporate new data incrementally:

```
log P(H|D) = log P(H) + Σ log P(Dk|H) - log Z
```

This is the natural fit for weather. Forecast models update at staggered intervals — ECMWF 2x daily, GFS 4x daily, HRRR hourly — so the bot receives a stream of evidence, not a single snapshot. Each model update is a new `Dk` that shifts the posterior. Log-space arithmetic avoids floating-point underflow when multiplying many small probabilities and makes the update a simple addition. The prior `P(H)` is the current belief; each model update nudges it rather than overwriting it, which smooths out noise from any single model run.

### Variant C: RL Agent (from @xmayeth)

@xmayeth's RL agent was born from a weather market loss (London temperature). The agent uses:
- **4 inputs**: order book state, time to resolution, volume, spread
- **4 actions**: YES, NO, hold, exit
- **Reward signal**: PnL

After 2M training iterations on 18 months of Polymarket data:
- Learned to avoid short-window markets (emerged, not programmed)
- Learned to time entries around volume spikes
- 67% win rate, 2.3 Sharpe on out-of-sample data

While the RL approach is general-purpose, weather markets were the catalyst and remain a strong fit because the input features (order book + time to resolution) capture the key dynamics of weather contracts resolving against deterministic outcomes.

## Key Data Sources for Implementation

### Free / Low-Cost
- **NOAA Forecast Grid API**: `https://api.weather.gov/gridpoints/{office}/{x},{y}/forecast` — free, hourly updates, includes probability distributions
- **GFS data**: Available via NOAA's NOMADS or AWS Open Data
- **HRRR data**: Available via NOAA's NOMADS or AWS Open Data (s3://noaa-hrrr-bdp-pds)

### Professional / Paid
- **ECMWF API**: Requires registration, some data free for research, commercial use requires license
- **UKMO DataPoint**: Free tier available with registration
- **CMC data**: Available through Canadian Meteorological Centre

### What to Ingest
For each forecast model, extract:
- **Point forecast** (expected temperature, rainfall, etc.)
- **Confidence intervals** (the key edge — markets often price the point forecast but ignore uncertainty)
- **Model agreement/disagreement** (when models diverge, one side is mispriced)
- **Forecast update timing** (know when new data drops to front-run market repricing)

## Risk Management

### Brier Score Calibration Monitoring

Weather forecast accuracy varies by season, geography, and weather regime. A model that's well-calibrated for summer temperatures in New York may be poorly calibrated for winter precipitation in Chicago. Brier score monitoring catches this drift:

```
Brier = Σ (p_predicted - outcome)^2
```

Where 0.00 = perfect, 0.25 = random guessing.

**Operational rules:**
- Check Brier score every **~50 trades**, stratified by market type (temperature, precipitation, wind, etc.)
- If Brier rises from ~0.12 to ~0.19, **halt trading** in that category — the forecast model's edge has degraded
- Seasonal recalibration: expect Brier to shift when weather regimes change (e.g., El Nino onset, seasonal transitions). Re-validate models at these boundaries.
- This is the only reliable way to distinguish "my weather model is good" from "I got lucky on a string of markets"

### VPIN Market Filter

Even in weather markets, informed trading exists. VPIN (Volume-Synchronized Probability of Informed Trading) catches surges in informed flow:

- Monitor VPIN continuously for active weather markets
- **VPIN > 0.6** → informed flow is elevated. Reduce position size or skip the market entirely
- In weather markets, VPIN spikes typically occur around extreme events when specialized traders (meteorologists, weather derivatives desks, reinsurance analysts) enter. This aligns with the alpha spikes noted in the opportunity section — VPIN is the real-time complement to the structural alpha metric.

### Position Sizing — Fractional Kelly

Weather forecasts carry meaningful uncertainty, even from professional models. Use **quarter-Kelly or half-Kelly**, never full Kelly:

```
f* = (p * b - q) / b     (then multiply by 0.25 or 0.5)
```

Rationale: forecast model disagreement is common (6 models rarely agree perfectly), so `p_hat` has wider confidence intervals than in, say, BTC latency arb where the "true" price is directly observable. Quarter-Kelly accounts for this estimation uncertainty while still scaling position size with edge.

### Black-Scholes for Time-Decay Weather Contracts

Some weather contracts have a meaningful time dimension — e.g., "Will temperature in London exceed 30C by July 15?" These behave like binary options with time decay. Black-Scholes binary pricing provides a theoretical floor/ceiling:

```
V = e^(-rT) * N(d2)
d2 = (ln(S/K) + (r - sigma^2/2) * T) / (sigma * sqrt(T))
```

Where `S` can be mapped to the current forecast value, `K` to the contract threshold, and `sigma` to the historical forecast error distribution. As `T → 0` (approaching resolution), the theoretical price collapses toward 0 or 1, and any remaining market price in the middle represents edge. This is most useful for longer-duration weather contracts (weekly, monthly) where time decay is a tradeable dynamic.

## Risks

- **Market liquidity**: Weather markets are thin. Getting in is easy; getting out at a reasonable price if you need to exit early may not be. Slippage on exit can eat the entire edge.
- **Resolution ambiguity**: While weather outcomes are "deterministic," the specific measurement station, time of reading, and data source Polymarket uses for resolution may not match the forecast model's reference point. A 0.1°C difference at the wrong station can flip a market.
- **Forecast model access reliability**: NOAA APIs have outages. ECMWF has rate limits and licensing restrictions. If the data feed goes down during a critical update cycle, the bot is flying blind with stale beliefs while the market may have already repriced.
- **Weather markets may dry up**: Polymarket creates weather markets because they drive engagement. If participation drops (or Polymarket pivots focus), the market supply could shrink. This is a platform-dependent opportunity, not a structural market feature.
- **Seasonal edge variation**: Forecast models are much better at some weather types than others. Summer temperature in flat terrain is highly predictable; winter precipitation in mountainous areas is notoriously difficult. The bot's edge will be seasonal and geographic — 70% win rate in summer doesn't mean 70% in winter.
- **Claimed returns are unverified marketing**: The claimed returns ($204→$24K, $9,685 overnight) are from Twitter threads promoting Telegram bots and referral links. The strategies may be real, but the specific numbers should be treated as marketing, not audited performance data. @hrundel75 links to a Polymarket wallet but the connection between the wallet and the tweeter is unverified.
- **Sharpe 12.96 is misleading**: Sharpe 12.96 from @hanakoxbt is almost certainly calculated over a very short window (one night, 78 trades). Annualized Sharpe ratios above 3-4 are extremely rare in any asset class. This number should not be used for capacity planning.
- **Small market size**: Even if the strategy works, weather markets on Polymarket may not have enough volume to deploy meaningful capital. $24K total profit over 1,516 trades implies ~$16 average profit per trade — this may not scale.

## High-Level Approach for RTT

### Universal Pipeline Mapping

Weather strategy plugs into the master strategy's universal decision pipeline. Steps 1-2 are weather-specific; steps 3-8 are shared infrastructure:

```
1. DATA INGEST     →  NOAA / ECMWF / GFS / HRRR / NAM / UKMO / CMC
2. FAIR VALUE      →  p_hat via Bayesian ensemble, updated each model cycle
3. EDGE DETECTION  →  edge = p_hat - p_market
4. MARKET FILTER   →  alpha < 0.12 AND VPIN < 0.6
5. POSITION SIZE   →  Quarter-Kelly, capped at 5% bankroll
6. EV GATE         →  gap > 0.15 (weather-specific threshold)
7. EXECUTE         →  CLOB order via Rust execution layer
8. MONITOR         →  Brier score every 50 trades (stratified by category), halt if degrading
```

### Phase 1: Single-Model MVP (Variant A)
1. Integrate NOAA Forecast Grid API (free, simple REST)
2. For each active weather market on Polymarket:
   - Parse the relevant forecast grid point
   - Extract probability distribution for the outcome buckets
   - Compare against market prices
   - If gap > 15%, generate a trade signal
3. Execute via existing RTT CLOB pipeline
4. Track Brier score for calibration monitoring

### Phase 2: Multi-Model Ensemble (Variant B)
1. Add GFS and HRRR ingestion (both free via AWS)
2. Build a consensus engine: for each weather market, compute the weighted average probability across models
3. When models agree but market disagrees → high-confidence trade
4. When models disagree → no trade (or reduce size)
5. Target 142ms or better for the full ingest → consensus → decision cycle

### Phase 3: Adaptive Entry Timing
1. Historical analysis of weather market order flow patterns
2. Learn optimal entry timing (volume spikes, pre-resolution patterns)
3. Could go full RL (Variant C) or use simpler heuristics

### Why Weather Markets
- **Low competition**: Fewer bots, wider spreads, less sophisticated counterparties
- **Data edge is real and legal**: Professional forecast data is freely available but most traders don't use it
- **Deterministic resolution**: No subjectivity, no governance risk
- **High win rates**: Both tracked strategies showed 70%+ win rates
- **Uncorrelated to crypto markets**: Provides portfolio diversification
- **The "boring" edge**: Nobody is excited about London temperature markets, which is exactly why they're mispriced

## Sanity Check

Before writing a spec or committing engineering effort, verify these assumptions:

- **Verify NOAA Forecast Grid API availability and format** by hitting the endpoint: `https://api.weather.gov/gridpoints/{office}/{x},{y}/forecast`. Check: does it return probability distributions or just point forecasts? If only point forecasts, the "confidence interval" edge described in Variant A may not be directly available from this API.
- **Verify which weather markets currently exist on Polymarket.** Are there enough active markets to make a dedicated strategy worthwhile? How frequently are new ones created?
- **Cross-check forecast model accuracy claims** by comparing NOAA/GFS/HRRR forecasts against actual weather station readings for a sample of 50+ events. Compute Brier scores for the models themselves before assuming they're reliable enough to trade on.
- **Test the 15% gap threshold** by backtesting against historical weather market resolutions. Is 15% the right threshold, or does the optimal threshold vary by market type (temperature vs. precipitation vs. wind)?
- **Verify that the 6 forecast models (@hanakoxbt's ensemble) are all freely accessible via API.** ECMWF in particular may require paid access for commercial use. If 3 of 6 models are behind paywalls, the ensemble approach may not be viable at the stated cost.
- **Verify resolution sources**: which weather station / data source does Polymarket use to resolve each market? If it's not the same source the forecast models predict for, there's basis risk.
- **Check whether weather markets have active reward pools** (relevant for combining this strategy with market maker rewards on the same markets).
- **Verify RL agent claims (Variant C)**: The RL agent claims 67% win rate and 2.3 Sharpe — verify these are on out-of-sample data, not training data. Also verify that 18 months of Polymarket data is enough for RL training (weather markets may not have existed for 18 months on Polymarket).
