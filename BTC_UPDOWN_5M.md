# BTC Up/Down 5-Minute Markets (Polymarket)

## Overview

Polymarket runs recurring 5-minute Bitcoin up/down markets. Each resolves "Up" if the BTC price at the end of the 5-min window is >= the price at the start, otherwise "Down". Resolution source is the [Chainlink BTC/USD data stream](https://data.chain.link/streams/btc-usd).

## Market Properties

| Property | Value |
|---|---|
| Series slug | `btc-up-or-down-5m` |
| Outcomes | `["Up", "Down"]` |
| Min order size | $5 |
| Tick size | 0.01 |
| Fees | maker 10bps / taker 10bps |
| negRisk | false |

## Slug Pattern

Each market's slug follows the pattern:

```
btc-updown-5m-{TIMESTAMP}
```

Where `{TIMESTAMP}` is a Unix epoch aligned to 5-minute boundaries (multiples of 300).

## Fetching Markets

### Get the next upcoming market

```bash
ts=$(( ($(date +%s) / 300 + 1) * 300 ))
curl -s "https://gamma-api.polymarket.com/events?slug=btc-updown-5m-$ts" | python3 -m json.tool
```

### Get the current market

```bash
ts=$(( ($(date +%s) / 300) * 300 ))
curl -s "https://gamma-api.polymarket.com/events?slug=btc-updown-5m-$ts" | python3 -m json.tool
```

### Extract just the token IDs

```bash
ts=$(( ($(date +%s) / 300 + 1) * 300 ))
curl -s "https://gamma-api.polymarket.com/events?slug=btc-updown-5m-$ts" | python3 -c "
import sys, json
e = json.load(sys.stdin)[0]
m = e['markets'][0]
tokens = json.loads(m['clobTokenIds'])
print(f'Title: {e[\"title\"]}')
print(f'Up:    {tokens[0]}')
print(f'Down:  {tokens[1]}')
print(f'Best bid: {m[\"bestBid\"]}  Best ask: {m[\"bestAsk\"]}')
"
```

## Timestamp Math

```
date +%s          → current Unix time in seconds (e.g. 1772684273)
/ 300             → integer divide by 300 (5 min = 300s) to get current slot index
+ 1               → advance to next slot (omit for current slot)
* 300             → multiply back to get the slot's start timestamp
```

Example:
```
now       = 1772684273
/ 300     = 5908947    (current slot)
+ 1       = 5908948    (next slot)
* 300     = 1772684400 (next market timestamp)
slug      = btc-updown-5m-1772684400
```

## Token ID Mapping

The `clobTokenIds` array in the API response is ordered:
- Index 0 → **Up** token
- Index 1 → **Down** token

Use these token IDs in `config.toml` under `[strategy]` → `token_id`.
