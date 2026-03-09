#!/usr/bin/env bash
set -euo pipefail

# Fire a single real order on a warm H2 connection and print latency stats.
#
# Usage:
#   ./scripts/fire.sh <token_id> [price] [fee_rate_bps] [neg_risk]
#
# First run builds the test binary (~3-5 min). Subsequent runs are instant.
# To force rebuild: rm .test_binary_path

MIN_ORDER_NOTIONAL="1.00"

compute_size() {
    local price="$1"

    awk -v price="$price" -v min_notional="$MIN_ORDER_NOTIONAL" '
        BEGIN {
            if ((price + 0) <= 0) {
                exit 1
            }

            required = min_notional / price
            size = int(required)
            if (required > size) {
                size += 1
            }
            if (size < 1) {
                size = 1
            }

            print size
        }
    '
}

compute_notional() {
    local price="$1"
    local size="$2"

    awk -v price="$price" -v size="$size" '
        BEGIN {
            printf "%.2f", price * size
        }
    '
}

find_stale_input() {
    local binary="$1"
    local path

    for path in Cargo.toml Cargo.lock crates/rtt-core/Cargo.toml scripts/fire.sh; do
        if [ -e "$path" ] && [ "$path" -nt "$binary" ]; then
            printf '%s\n' "$path"
            return 0
        fi
    done

    local newer_file
    newer_file="$(find crates/rtt-core/src crates/rtt-core/tests -type f -newer "$binary" -print -quit 2>/dev/null || true)"
    if [ -n "$newer_file" ]; then
        printf '%s\n' "$newer_file"
        return 0
    fi

    return 1
}

warn_if_stale_binary() {
    local binary="$1"
    local stale_input

    stale_input="$(find_stale_input "$binary" || true)"
    if [ -z "$stale_input" ]; then
        return 0
    fi

    cat >&2 <<EOF
WARNING: cached rtt-core test binary appears stale.
Newest detected input: $stale_input

Refresh commands:
  rm -f .test_binary_path
  cargo test --release -p rtt-core --no-run

Then rerun:
  ./scripts/fire.sh <token_id> [price] [fee_rate_bps] [neg_risk]
EOF
}

main() {
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
    SIZE="$(compute_size "$PRICE")" || {
        echo "Error: invalid price '$PRICE'" >&2
        exit 1
    }
    NOTIONAL="$(compute_notional "$PRICE" "$SIZE")"

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
    export SIZE
    export FEE_RATE_BPS
    export NEG_RISK
    export SIG_TYPE=2  # GNOSIS_SAFE (proxy wallet as maker, EOA as signer)

    # Build test binary once, cache the path
    CACHE_FILE=".test_binary_path"
    if [ ! -f "$CACHE_FILE" ] || [ ! -f "$(cat "$CACHE_FILE" 2>/dev/null)" ]; then
        echo "Building test binary (one-time)..."
        BINARY=$(cargo test --release -p rtt-core --no-run --message-format=json 2>/dev/null \
            | grep '"executable"' \
            | grep 'rtt.core' \
            | tail -1 \
            | sed 's/.*"executable":"\([^"]*\)".*/\1/')
        if [ -z "$BINARY" ]; then
            echo "Error: could not find test binary"
            exit 1
        fi
        echo "$BINARY" > "$CACHE_FILE"
        echo "Cached: $BINARY"
    fi

    BINARY=$(cat "$CACHE_FILE")
    warn_if_stale_binary "$BINARY"

    echo "=== fire.sh ==="
    echo "token_id:      ${TOKEN_ID:0:12}...${TOKEN_ID: -6}"
    echo "price:         $PRICE"
    echo "size:          $SIZE (derived)"
    echo "notional:      \$$NOTIONAL"
    echo "type:          FOK"
    echo "fee_rate_bps:  $FEE_RATE_BPS"
    echo "neg_risk:      $NEG_RISK"
    echo ""

    "$BINARY" test_clob_end_to_end_pipeline --ignored --nocapture
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
