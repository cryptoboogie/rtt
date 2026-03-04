use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::fmt;

// ---------------------------------------------------------------------------
// CircuitBreaker
// ---------------------------------------------------------------------------

/// Error returned when the circuit breaker has tripped.
#[derive(Debug, Clone)]
pub struct CircuitBreakerTripped {
    pub reason: String,
}

impl fmt::Display for CircuitBreakerTripped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Circuit breaker tripped: {}", self.reason)
    }
}

impl std::error::Error for CircuitBreakerTripped {}

/// Lock-free circuit breaker that tracks order count and USD exposure.
///
/// Once tripped, it stays tripped for the lifetime of the process.
/// This is intentional — a restart is required to reset.
#[derive(Clone)]
pub struct CircuitBreaker {
    orders_fired: Arc<AtomicU64>,
    usd_committed_cents: Arc<AtomicU64>,
    max_orders: u64,
    max_usd_cents: u64,
    tripped: Arc<AtomicBool>,
}

impl CircuitBreaker {
    pub fn new(max_orders: u64, max_usd: f64) -> Self {
        Self {
            orders_fired: Arc::new(AtomicU64::new(0)),
            usd_committed_cents: Arc::new(AtomicU64::new(0)),
            max_orders,
            max_usd_cents: (max_usd * 100.0) as u64,
            tripped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check limits and record an order atomically.
    /// Returns Err if the breaker trips (either from this call or previously).
    pub fn check_and_record(&self, price: &str, size: &str) -> Result<(), CircuitBreakerTripped> {
        if self.tripped.load(Ordering::Acquire) {
            return Err(CircuitBreakerTripped {
                reason: "already tripped".to_string(),
            });
        }

        // Parse price and size, compute USD value in cents
        let price_f: f64 = price.parse().unwrap_or(0.0);
        let size_f: f64 = size.parse().unwrap_or(0.0);
        let usd_cents = (price_f * size_f * 100.0) as u64;

        // Atomically increment order count
        let prev_orders = self.orders_fired.fetch_add(1, Ordering::AcqRel);
        if prev_orders + 1 > self.max_orders {
            self.tripped.store(true, Ordering::Release);
            return Err(CircuitBreakerTripped {
                reason: format!(
                    "max orders exceeded: {}/{}",
                    prev_orders + 1,
                    self.max_orders
                ),
            });
        }

        // Atomically increment USD committed
        let prev_usd = self.usd_committed_cents.fetch_add(usd_cents, Ordering::AcqRel);
        if prev_usd + usd_cents > self.max_usd_cents {
            self.tripped.store(true, Ordering::Release);
            return Err(CircuitBreakerTripped {
                reason: format!(
                    "max USD exceeded: ${:.2}/${:.2}",
                    (prev_usd + usd_cents) as f64 / 100.0,
                    self.max_usd_cents as f64 / 100.0,
                ),
            });
        }

        Ok(())
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    /// Returns (orders_fired, usd_committed).
    pub fn stats(&self) -> (u64, f64) {
        let orders = self.orders_fired.load(Ordering::Relaxed);
        let usd_cents = self.usd_committed_cents.load(Ordering::Relaxed);
        (orders, usd_cents as f64 / 100.0)
    }

    /// Manually trip the breaker (e.g., on server error).
    pub fn trip(&self) {
        self.tripped.store(true, Ordering::Release);
    }

    pub fn max_orders(&self) -> u64 {
        self.max_orders
    }

    pub fn max_usd(&self) -> f64 {
        self.max_usd_cents as f64 / 100.0
    }
}

// ---------------------------------------------------------------------------
// RateLimiter
// ---------------------------------------------------------------------------

/// Lock-free sliding-window rate limiter.
///
/// If the rate is exceeded, triggers are dropped (not queued).
pub struct RateLimiter {
    max_per_second: u64,
    window_start_ns: AtomicU64,
    count_in_window: AtomicU64,
}

impl RateLimiter {
    pub fn new(max_per_second: u64) -> Self {
        Self {
            max_per_second,
            window_start_ns: AtomicU64::new(0),
            count_in_window: AtomicU64::new(0),
        }
    }

    /// Returns true if under the rate limit, false if exceeded.
    pub fn try_acquire(&self) -> bool {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let window_start = self.window_start_ns.load(Ordering::Acquire);
        let elapsed_ns = now_ns.saturating_sub(window_start);

        // If more than 1 second has passed, reset the window
        if elapsed_ns >= 1_000_000_000 {
            self.window_start_ns.store(now_ns, Ordering::Release);
            self.count_in_window.store(1, Ordering::Release);
            return true;
        }

        // Increment count and check
        let prev = self.count_in_window.fetch_add(1, Ordering::AcqRel);
        if prev + 1 > self.max_per_second {
            // Over limit — don't undo the increment (harmless, resets next window)
            return false;
        }

        true
    }
}

// ---------------------------------------------------------------------------
// OrderGuard
// ---------------------------------------------------------------------------

/// Ensures only one order is in flight at a time.
#[derive(Clone)]
pub struct OrderGuard {
    in_flight: Arc<AtomicBool>,
}

impl OrderGuard {
    pub fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns true if no order was in flight (and sets the flag).
    /// Returns false if an order is already in flight.
    pub fn try_acquire(&self) -> bool {
        // compare_exchange: if currently false, set to true
        self.in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Releases the in-flight flag.
    pub fn release(&self) {
        self.in_flight.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- CircuitBreaker tests ---

    #[test]
    fn circuit_breaker_fires_up_to_max_orders_then_trips() {
        let cb = CircuitBreaker::new(3, 1000.0);

        assert!(cb.check_and_record("0.50", "10").is_ok());
        assert!(cb.check_and_record("0.50", "10").is_ok());
        assert!(cb.check_and_record("0.50", "10").is_ok());
        // 4th should trip
        assert!(cb.check_and_record("0.50", "10").is_err());
        assert!(cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_fires_up_to_max_usd_then_trips() {
        // max $10, each order costs $5 (price=0.50 * size=10)
        let cb = CircuitBreaker::new(100, 10.0);

        assert!(cb.check_and_record("0.50", "10").is_ok()); // $5 committed
        assert!(cb.check_and_record("0.50", "10").is_ok()); // $10 committed
        // $15 would exceed $10 limit
        assert!(cb.check_and_record("0.50", "10").is_err());
        assert!(cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_once_tripped_all_subsequent_fail() {
        let cb = CircuitBreaker::new(1, 1000.0);

        assert!(cb.check_and_record("0.50", "10").is_ok());
        assert!(cb.check_and_record("0.50", "10").is_err()); // trips
        // All subsequent
        assert!(cb.check_and_record("0.01", "1").is_err());
        assert!(cb.check_and_record("0.01", "1").is_err());
    }

    #[test]
    fn circuit_breaker_manual_trip() {
        let cb = CircuitBreaker::new(100, 1000.0);

        assert!(!cb.is_tripped());
        cb.trip();
        assert!(cb.is_tripped());
        assert!(cb.check_and_record("0.50", "10").is_err());
    }

    #[test]
    fn circuit_breaker_stats() {
        let cb = CircuitBreaker::new(100, 1000.0);

        cb.check_and_record("0.50", "10").unwrap(); // $5
        cb.check_and_record("0.30", "20").unwrap(); // $6

        let (orders, usd) = cb.stats();
        assert_eq!(orders, 2);
        assert!((usd - 11.0).abs() < 0.01);
    }

    #[test]
    fn circuit_breaker_thread_safe() {
        let cb = CircuitBreaker::new(1000, 10000.0);
        let mut handles = vec![];

        for _ in 0..10 {
            let cb_clone = cb.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let _ = cb_clone.check_and_record("0.01", "1");
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let (orders, _) = cb.stats();
        assert_eq!(orders, 1000);
    }

    // --- RateLimiter tests ---

    #[test]
    fn rate_limiter_allows_up_to_max_per_second() {
        let rl = RateLimiter::new(5);

        for _ in 0..5 {
            assert!(rl.try_acquire());
        }
    }

    #[test]
    fn rate_limiter_rejects_after_limit() {
        let rl = RateLimiter::new(2);

        assert!(rl.try_acquire());
        assert!(rl.try_acquire());
        assert!(!rl.try_acquire());
    }

    #[test]
    fn rate_limiter_resets_after_window() {
        let rl = RateLimiter::new(1);

        assert!(rl.try_acquire());
        assert!(!rl.try_acquire());

        // Wait for window to pass
        std::thread::sleep(std::time::Duration::from_millis(1100));

        assert!(rl.try_acquire());
    }

    // --- OrderGuard tests ---

    #[test]
    fn order_guard_first_acquire_succeeds() {
        let guard = OrderGuard::new();
        assert!(guard.try_acquire());
    }

    #[test]
    fn order_guard_second_acquire_fails_while_held() {
        let guard = OrderGuard::new();
        assert!(guard.try_acquire());
        assert!(!guard.try_acquire());
    }

    #[test]
    fn order_guard_acquire_after_release_succeeds() {
        let guard = OrderGuard::new();
        assert!(guard.try_acquire());
        guard.release();
        assert!(guard.try_acquire());
    }
}
