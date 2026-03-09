# Research: Market Maker Rewards Strategy

## Overview

Polymarket incentivizes liquidity provision through a reward program that distributes USDC rebates to market makers. This strategy is **zero-directional-risk** — it requires no price prediction, no ML models, and no data feeds beyond the Polymarket API itself. It is the simplest profitable strategy to implement.

## Sources

| Author | Handle | Key Contribution |
|---|---|---|
| Discover | @0x_discover | Full reward farming strategy, market selection criteria |
| BuBBliK | @k1rallik | Fee expansion to all crypto timeframes, rebate structure |
| self.dll | @seelffff | Adverse selection formula for market screening |

## Source Links (actually accessed)

- https://api.fxtwitter.com/0x_discover/status/2029266629033639981 — reward farming strategy, market selection, dual-sided orders
- https://api.fxtwitter.com/k1rallik/status/2029887946094981301 — fee expansion to all crypto timeframes, rebate structure, 20% maker rebates
- https://api.fxtwitter.com/seelffff/status/2030310382020001936 — adverse selection alpha formula, spread-as-signal
- https://api.fxtwitter.com/rohonchain/status/2029998336837890193 — hedge fund breakdown; informed VPIN, longshot bias, conditional arbitrage sections
- https://api.fxtwitter.com/polydao/status/2030029152997245077 — LMSR/whitepaper decode; informed the LMSR pricing section

## The Opportunity

### How Rewards Work
- Polymarket charges **taker fees** on crypto markets across all timeframes (5m, 15m, 1H, 4H, daily, weekly)
- **20% of all collected fees** are redistributed to makers as **daily USDC rebates**
- Rebates are calculated **per market** — you only compete with other makers in the same market
- Peak effective fee: **1.56% at 50c**, tapering to near-zero at price extremes

### LMSR Pricing and What It Means for Market Makers

Polymarket prices outcomes using the **Logarithmic Market Scoring Rule** (LMSR), where displayed probabilities are a softmax over the outcome share vector:

```
pi = e^(qi/b) / Σ e^(qj/b)
```

This is the same math as a neural network output layer — the parameter `b` (the liquidity parameter) controls how sensitive prices are to share purchases. The key insight for market makers: **price sensitivity to `b` peaks near p = 0.50**. This is where:

- The cost function curve is steepest, meaning small share purchases move the price the most
- Taker fees peak at **1.56%**, generating the largest rebate pool
- Maker competition is also highest, because everyone knows this

In practice, markets near 50/50 are the most lucrative *per dollar of fee collected* but also the most crowded. Markets at 20-30c or 70-80c offer a better competition-adjusted reward ratio — the fee is lower but you may be the only maker.

### Why Now
Per @k1rallik, Polymarket recently expanded taker fees from just 5m/15m markets to **all crypto market timeframes**. This significantly increased the rebate pool. Participation is still thin — early-mover advantage is real.

## The Strategy

From @0x_discover, the strategy is dead simple:

1. **Scan** for markets with active reward pools and wide spreads
2. **Place dual-sided limit orders**: YES and NO simultaneously
3. **Three possible outcomes**:
   - **No fills** — orders sit in the book, rewards accumulate from resting liquidity
   - **One side fills** — bot instantly hedges the other side. If YES + NO cost ≤ $1.02, profit regardless of outcome
   - **Both sides fill** — fully hedged, one side settles at $1, clean close

### The Edge: Market Selection

The strategy performs best in **overlooked, low-competition markets**: primaries, minor leagues, regional referendums. Less competition means a bigger share of the reward pool.

Critical screening criteria:
- Active reward pool (not all markets have one)
- Spread wide enough to capture but not so wide it signals informed trading
- Low maker competition (fewer resting orders = larger rebate share)

## Adverse Selection Filter

From @seelffff, the most important risk management insight for market makers:

```
alpha = Delta / (V_h - mu)
```

Where `alpha` = estimated fraction of **informed traders** in a market.

| Spread | Alpha | Signal |
|---|---|---|
| 0.05 | ~5% | Safe to enter |
| 0.15 | ~15% | Be careful |
| 0.25 | ~20%+ | One in five traders knows the outcome — avoid |

**Rule: Do not provide liquidity in any market where `alpha > 12%`.**

The counterintuitive insight: **wide spreads do NOT mean inefficient/exploitable markets**. They mean informed traders (smart money) are present and the market maker is protecting against adverse selection. A market maker entering a high-alpha market is paying someone else's information edge.

### VPIN — Complementary Real-Time Signal

Alpha is a structural metric — it tells you about the *steady-state* composition of a market's participants. **VPIN** (Volume-Synchronized Probability of Informed Trading) is a *dynamic* signal that catches surges in informed flow as they happen:

- Monitor VPIN continuously for every market where orders are resting
- **VPIN > 0.6** → informed trading volume is elevated. Widen spreads or pull orders entirely until VPIN drops back below threshold
- VPIN spikes often precede adverse selection events — someone is trading on information you don't have

The alpha filter decides *which markets to enter*. VPIN decides *when to step back from a market you're already in*. Both are required for a production market maker.

## Profit Mechanics

### Revenue Streams
1. **Rebate income**: 20% of taker fees redistributed proportionally to resting maker volume
2. **Spread capture**: When both sides fill, the market maker captures the spread minus $1.00 (or $0.00 for the losing side)
3. **Favorable resolution**: Sometimes one side fills and the market resolves favorably without needing to hedge

### Cost/Risk
- **Capital lockup**: Funds are tied in resting orders and open positions
- **Adverse selection**: Informed traders fill your orders right before resolution (mitigated by alpha filter + VPIN)
- **Inventory risk**: One side fills but hedging the other side moves the market against you
- **Spread compression**: More makers enter, spreads tighten, rewards per maker decrease

### Longshot Bias and Book Asymmetry

From the hedge fund research: markets priced **below 15%** are systematically overpriced due to **longshot bias** — people overpay for unlikely outcomes. This creates a structural edge for the market maker's book management:

- When providing liquidity in low-probability markets (<15%), **lean toward the NO side**. Takers disproportionately buy YES (overpaying for the longshot), so your NO orders fill more and resolve profitably more often.
- When providing liquidity in high-probability markets (>85%), the inverse applies — lean toward YES.
- For markets near 50%, no structural lean; the book should be symmetric.

This isn't directional betting — it's a statistical adjustment to which side of your book you expose more capital to, based on a well-documented pricing anomaly.

### Brier Score Monitoring for One-Sided Fills

Pure market making doesn't require prediction, so Brier score calibration is not directly applicable. However, **once the bot starts accumulating unhedged one-sided fills**, it is implicitly taking directional positions. In that scenario:

- Track the outcomes of unhedged positions as if they were directional bets
- Compute a Brier score over rolling windows (~50 resolved positions)
- If Brier score degrades (rises above ~0.19), the bot's inventory management is systematically landing on the wrong side — tighten hedge triggers or reduce order sizes
- This converts what would be invisible P&L drag into a measurable, actionable signal

### Conditional Arbitrage Opportunities

A market maker monitoring multiple related markets is uniquely positioned to spot **logical constraint violations** — cases where probabilities across related markets are inconsistent. Examples:

- P("BTC > $100K this week") should be ≤ P("BTC > $100K this month")
- P("A wins AND B wins") can't exceed P("A wins")
- Multi-outcome markets (e.g., "Who wins the election?") must sum to ~1.00

When the bot detects a violation, it can take a **directional position with near-zero risk**: buy the underpriced side and sell the overpriced side. This is not market making — it's arbitrage that falls out naturally from monitoring the same order books the bot already watches.

These opportunities are rare but high-conviction. The bot should flag them for execution alongside its normal maker activity.

## Risks

- **Adverse selection is the #1 killer.** Even with the alpha filter and VPIN, informed traders can and will pick off stale quotes. The bot's resting orders are visible to everyone — sophisticated takers can model which markets the bot is in and target it.
- **Reward pool changes.** Polymarket controls the reward program parameters. The 20% rebate rate, per-market calculation, and pool sizes can change at any time with no notice. The strategy's profitability is partially dependent on a centralized entity's policy decisions.
- **Capital efficiency.** Funds locked in resting orders earn no yield. In a high-interest-rate environment, the opportunity cost of capital is real. Compare expected rebate + spread income against risk-free USDC yield (e.g., Aave, T-bills).
- **Spread compression from competition.** As more makers enter (especially if this strategy becomes widely known from these exact tweets), spreads will tighten and reward shares will shrink. First-mover advantage is real but temporary.
- **Inventory risk on one-sided fills.** If one side fills and the market moves against you before you can hedge, the loss can exceed the rebate income. In fast-moving markets (crypto especially), this can happen in seconds.
- **Smart contract / platform risk.** Funds deposited on Polymarket are subject to smart contract bugs, platform downtime, or regulatory action. This is not a risk-free strategy — it's a low-directional-risk strategy with platform counterparty risk.
- **Promotional claims.** The @0x_discover claim of "built in 2 hours, $3,000 overnight" is almost certainly marketing for the linked Telegram bot (KreoPolyBot). The strategy concept is sound but the stated returns are likely promotional.

## High-Level Approach for RTT

### Phase 1: Market Scanner
1. Query Polymarket API for all markets with active reward pools
2. For each market, compute:
   - Current spread (bid-ask)
   - Resting maker volume (competition level)
   - `alpha` (adverse selection metric)
3. Rank markets by `reward_pool_size / maker_competition` ratio
4. Filter out markets where `alpha > 12%`

### Phase 2: Order Management
1. For qualifying markets, place symmetrical YES/NO limit orders
2. Price orders to capture spread while ensuring YES + NO cost ≤ $1.00 + acceptable_margin
3. Monitor fills in real-time via WebSocket
4. On single-side fill: immediately hedge the other side if favorable, or let it ride if cost constraints are met

### Phase 3: Position Management
1. Track all open positions and their hedge status
2. For fully hedged positions: hold until resolution (guaranteed profit)
3. For partially hedged positions: monitor and decide whether to hedge or exit
4. Sweep rebate income daily

### Why This Strategy First
- **Simplest to implement**: No ML, no external data feeds, no latency requirements
- **Zero prediction risk**: The strategy doesn't need to be right about outcomes
- **Immediate revenue**: Rebates accrue from day one, even without fills
- **Low capital requirement**: @0x_discover claims it was built in 2 hours and started generating income immediately
- **Compounds well**: Rebate income can be reinvested into more resting orders
- **Risk is bounded**: Maximum loss per position is the spread cost

## Sanity Check

- Verify the **20% rebate rate** and per-market calculation by checking Polymarket's official docs and reward program announcements. This is the foundation of the strategy — if the rate is lower or the calculation is different, the economics change dramatically.
- Verify which markets actually have **active reward pools**. Not all markets qualify. Build a script to query the Polymarket API and list markets with active reward programs before assuming broad applicability.
- Test the **YES + NO <= $1.02** assumption by checking actual order books. If the combined cost is consistently above $1.02, the hedged-fill profit disappears. Wider spreads make this easier but also signal higher alpha.
- Verify the **alpha formula** (`alpha = Delta / (V_h - mu)`) by computing it for a sample of markets and checking whether high-alpha markets actually have worse maker outcomes. The formula comes from a single Twitter thread — validate it empirically before building it into the system.
- Cross-check **rebate income against capital lockup costs**. If $10K locked in orders earns $50/day in rebates, that's 0.5%/day or ~182% APY — sounds great. But if it's $5/day, that's 18% APY, which may not justify the smart contract risk. Get real numbers.
- Verify that the **longshot bias claim** (markets <15% are overpriced) holds on Polymarket specifically. This is well-documented in sports betting and horse racing but may not transfer to prediction markets with different participant demographics.
- Check Polymarket's **current maker/taker fee structure**. The document assumes makers are fee-free — verify this hasn't changed.
- The **conditional arbitrage opportunity** (logical constraint violations) should be tested by monitoring related markets for a week and logging any violations found. If none appear, this is theoretical, not practical.
