use std::time::Instant;
use std::sync::OnceLock;

static EPOCH: OnceLock<Instant> = OnceLock::new();

fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

/// Returns monotonic nanoseconds since process start.
pub fn now_ns() -> u64 {
    epoch().elapsed().as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_returns_value() {
        // First call initializes epoch, so elapsed may be 0.
        // Second call after a spin should be > 0.
        let _ = now_ns();
        std::thread::sleep(std::time::Duration::from_micros(1));
        let t = now_ns();
        assert!(t > 0);
    }

    #[test]
    fn is_monotonic() {
        let a = now_ns();
        let b = now_ns();
        let c = now_ns();
        assert!(b >= a);
        assert!(c >= b);
    }

    #[test]
    fn sub_millisecond_resolution() {
        let a = now_ns();
        // Busy-spin briefly
        for _ in 0..1000 {
            std::hint::black_box(0);
        }
        let b = now_ns();
        let delta = b - a;
        // Should be < 1ms (1_000_000 ns)
        assert!(delta < 1_000_000, "delta was {} ns", delta);
    }

    #[test]
    fn measures_real_time() {
        let a = now_ns();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let b = now_ns();
        let delta = b - a;
        // Should be at least 5ms (allowing for sleep imprecision)
        assert!(delta >= 5_000_000, "delta was {} ns", delta);
        // Should be less than 100ms
        assert!(delta < 100_000_000, "delta was {} ns", delta);
    }
}
