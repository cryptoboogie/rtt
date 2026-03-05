#!/usr/bin/env bash
set -euo pipefail

# Fire a single real order on a warm H2 connection and print latency stats.
#
# Usage:
#   ./scripts/fire.sh <token_id> [price]
#
# Example:
#   ./scripts/fire.sh 12345678901234567890 0.95
#
# Defaults: price=0.95, size=2 ($1.90), side=Buy, FOK
# Credentials loaded from .env (POLY_API_KEY, POLY_SECRET, etc.)

if [ $# -lt 1 ]; then
    echo "Usage: ./scripts/fire.sh <token_id> [price] [fee_rate_bps] [neg_risk]"
    echo ""
    echo "  token_id       The condition token ID to buy"
    echo "  price          Bid price (default: 0.95)"
    echo "  fee_rate_bps   Taker fee in bps (default: 1000 = 10%)"
    echo "  neg_risk       true/false (default: false)"
    exit 1
fi

TOKEN_ID="$1"
PRICE="${2:-0.95}"
FEE_RATE_BPS="${3:-1000}"
NEG_RISK="${4:-false}"

cd "$(dirname "$0")/.."

if [ ! -f .env ]; then
    echo "Error: .env not found in $(pwd)"
    exit 1
fi

set -a
source .env
set +a

export TOKEN_ID
export PRICE
export FEE_RATE_BPS
export NEG_RISK

echo "=== fire.sh ==="
echo "token_id:      ${TOKEN_ID:0:12}...${TOKEN_ID: -6}"
echo "price:         $PRICE"
echo "size:          2 (hardcoded)"
echo "type:          FOK"
echo "fee_rate_bps:  $FEE_RATE_BPS"
echo "neg_risk:      $NEG_RISK"
echo ""

cargo test --release -p rtt-core test_clob_end_to_end_pipeline -- --ignored --nocapture
