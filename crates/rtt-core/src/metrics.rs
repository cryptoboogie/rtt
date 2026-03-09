/// Per-request timestamp record with 8 checkpoints.
#[derive(Debug, Clone, Default)]
pub struct TimestampRecord {
    pub t_trigger_rx: u64,
    pub t_dispatch_q: u64,
    pub t_exec_start: u64,
    pub t_buf_ready: u64,
    pub t_write_begin: u64,
    pub t_write_end: u64,
    pub t_first_resp_byte: u64,
    pub t_headers_done: u64,
    pub t_sign_start: u64,
    pub t_sign_end: u64,
    pub is_reconnect: bool,
    pub cf_ray_pop: String,
    pub connection_index: usize,
}

impl TimestampRecord {
    /// Time spent in queue: exec_start - trigger_rx
    pub fn queue_delay(&self) -> u64 {
        self.t_exec_start.saturating_sub(self.t_trigger_rx)
    }

    /// Time to prepare request buffer: buf_ready - exec_start
    pub fn prep_time(&self) -> u64 {
        self.t_buf_ready.saturating_sub(self.t_exec_start)
    }

    /// Trigger-to-wire latency: write_begin - trigger_rx
    pub fn trigger_to_wire(&self) -> u64 {
        self.t_write_begin.saturating_sub(self.t_trigger_rx)
    }

    /// Write duration: write_end - write_begin
    pub fn write_duration(&self) -> u64 {
        self.t_write_end.saturating_sub(self.t_write_begin)
    }

    /// Time from write end to first response byte
    pub fn write_to_first_byte(&self) -> u64 {
        self.t_first_resp_byte.saturating_sub(self.t_write_end)
    }

    /// Warm TTFB: first_resp_byte - write_begin
    pub fn warm_ttfb(&self) -> u64 {
        self.t_first_resp_byte.saturating_sub(self.t_write_begin)
    }

    /// Total trigger-to-first-byte: first_resp_byte - trigger_rx
    pub fn trigger_to_first_byte(&self) -> u64 {
        self.t_first_resp_byte.saturating_sub(self.t_trigger_rx)
    }

    /// EIP-712 signing duration: sign_end - sign_start
    pub fn sign_duration(&self) -> u64 {
        self.t_sign_end.saturating_sub(self.t_sign_start)
    }
}

/// Percentile set for a single metric.
#[derive(Debug, Clone, Default)]
pub struct PercentileSet {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub p999: u64,
    pub max: u64,
}

/// Full stats report across all 7 derived metrics.
#[derive(Debug, Clone, Default)]
pub struct StatsReport {
    pub sample_count: usize,
    pub reconnect_count: usize,
    pub queue_delay: PercentileSet,
    pub prep_time: PercentileSet,
    pub trigger_to_wire: PercentileSet,
    pub write_duration: PercentileSet,
    pub write_to_first_byte: PercentileSet,
    pub warm_ttfb: PercentileSet,
    pub trigger_to_first_byte: PercentileSet,
}

/// Collects TimestampRecords, filters reconnects, computes percentiles.
#[derive(Debug, Default)]
pub struct StatsAggregator {
    records: Vec<TimestampRecord>,
}

impl StatsAggregator {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub fn add(&mut self, record: TimestampRecord) {
        self.records.push(record);
    }

    pub fn records(&self) -> &[TimestampRecord] {
        &self.records
    }

    pub fn compute(&self) -> StatsReport {
        let reconnect_count = self.records.iter().filter(|r| r.is_reconnect).count();
        let warm: Vec<&TimestampRecord> = self.records.iter().filter(|r| !r.is_reconnect).collect();
        let sample_count = warm.len();

        if sample_count == 0 {
            return StatsReport {
                sample_count: 0,
                reconnect_count,
                ..Default::default()
            };
        }

        let extract =
            |f: fn(&TimestampRecord) -> u64| -> Vec<u64> { warm.iter().map(|r| f(r)).collect() };

        StatsReport {
            sample_count,
            reconnect_count,
            queue_delay: percentiles(&mut extract(TimestampRecord::queue_delay)),
            prep_time: percentiles(&mut extract(TimestampRecord::prep_time)),
            trigger_to_wire: percentiles(&mut extract(TimestampRecord::trigger_to_wire)),
            write_duration: percentiles(&mut extract(TimestampRecord::write_duration)),
            write_to_first_byte: percentiles(&mut extract(TimestampRecord::write_to_first_byte)),
            warm_ttfb: percentiles(&mut extract(TimestampRecord::warm_ttfb)),
            trigger_to_first_byte: percentiles(&mut extract(
                TimestampRecord::trigger_to_first_byte,
            )),
        }
    }
}

fn percentiles(values: &mut Vec<u64>) -> PercentileSet {
    if values.is_empty() {
        return PercentileSet::default();
    }
    values.sort_unstable();
    let n = values.len();
    PercentileSet {
        p50: values[percentile_index(n, 50.0)],
        p95: values[percentile_index(n, 95.0)],
        p99: values[percentile_index(n, 99.0)],
        p999: values[percentile_index(n, 99.9)],
        max: values[n - 1],
    }
}

fn percentile_index(n: usize, p: f64) -> usize {
    let idx = (p / 100.0 * (n as f64 - 1.0)).round() as usize;
    idx.min(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(base: u64, reconnect: bool) -> TimestampRecord {
        TimestampRecord {
            t_trigger_rx: base,
            t_dispatch_q: base + 100,
            t_exec_start: base + 200,
            t_buf_ready: base + 300,
            t_write_begin: base + 400,
            t_write_end: base + 500,
            t_first_resp_byte: base + 1000,
            t_headers_done: base + 1100,
            t_sign_start: 0,
            t_sign_end: 0,
            is_reconnect: reconnect,
            cf_ray_pop: String::new(),
            connection_index: 0,
        }
    }

    #[test]
    fn connection_index_defaults_to_zero() {
        let r = TimestampRecord::default();
        assert_eq!(r.connection_index, 0);
    }

    #[test]
    fn connection_index_set_and_read() {
        let mut r = make_record(0, false);
        r.connection_index = 1;
        assert_eq!(r.connection_index, 1);
    }

    #[test]
    fn queue_delay() {
        let r = make_record(1000, false);
        assert_eq!(r.queue_delay(), 200);
    }

    #[test]
    fn prep_time() {
        let r = make_record(1000, false);
        assert_eq!(r.prep_time(), 100);
    }

    #[test]
    fn trigger_to_wire() {
        let r = make_record(1000, false);
        assert_eq!(r.trigger_to_wire(), 400);
    }

    #[test]
    fn write_duration() {
        let r = make_record(1000, false);
        assert_eq!(r.write_duration(), 100);
    }

    #[test]
    fn write_to_first_byte() {
        let r = make_record(1000, false);
        assert_eq!(r.write_to_first_byte(), 500);
    }

    #[test]
    fn warm_ttfb() {
        let r = make_record(1000, false);
        assert_eq!(r.warm_ttfb(), 600);
    }

    #[test]
    fn trigger_to_first_byte() {
        let r = make_record(1000, false);
        assert_eq!(r.trigger_to_first_byte(), 1000);
    }

    #[test]
    fn sign_duration() {
        let mut r = make_record(1000, false);
        r.t_sign_start = 5000;
        r.t_sign_end = 5500;
        assert_eq!(r.sign_duration(), 500);
    }

    #[test]
    fn sign_duration_defaults_to_zero() {
        let r = TimestampRecord::default();
        assert_eq!(r.sign_duration(), 0);
    }

    #[test]
    fn saturating_sub_on_zero() {
        let r = TimestampRecord::default();
        assert_eq!(r.queue_delay(), 0);
        assert_eq!(r.trigger_to_wire(), 0);
    }

    #[test]
    fn all_derived_metrics_consistent() {
        let r = make_record(0, false);
        // trigger_to_first_byte = queue_delay + prep_time + (write_begin - buf_ready) + write_duration + write_to_first_byte
        assert_eq!(
            r.trigger_to_first_byte(),
            r.queue_delay()
                + r.prep_time()
                + (r.t_write_begin - r.t_buf_ready)
                + r.write_duration()
                + r.write_to_first_byte()
        );
    }

    // Stats aggregator tests
    #[test]
    fn stats_empty() {
        let agg = StatsAggregator::new();
        let report = agg.compute();
        assert_eq!(report.sample_count, 0);
    }

    #[test]
    fn stats_filters_reconnects() {
        let mut agg = StatsAggregator::new();
        agg.add(make_record(1000, false));
        agg.add(make_record(2000, true));
        agg.add(make_record(3000, false));
        let report = agg.compute();
        assert_eq!(report.sample_count, 2);
        assert_eq!(report.reconnect_count, 1);
    }

    #[test]
    fn stats_percentiles_single_sample() {
        let mut agg = StatsAggregator::new();
        agg.add(make_record(0, false));
        let report = agg.compute();
        assert_eq!(report.queue_delay.p50, 200);
        assert_eq!(report.queue_delay.p99, 200);
        assert_eq!(report.queue_delay.max, 200);
    }

    #[test]
    fn stats_percentiles_multiple_samples() {
        let mut agg = StatsAggregator::new();
        // Add 100 samples with varying base times to get spread
        for i in 0..100u64 {
            let mut rec = make_record(i * 10, false);
            // Vary exec_start to get different queue_delay values
            rec.t_exec_start = rec.t_trigger_rx + (i + 1) * 100;
            agg.add(rec);
        }
        let report = agg.compute();
        assert_eq!(report.sample_count, 100);
        // p50 should be around the median queue_delay
        assert!(report.queue_delay.p50 > 0);
        assert!(report.queue_delay.p99 > report.queue_delay.p50);
        assert!(report.queue_delay.max >= report.queue_delay.p99);
    }

    #[test]
    fn stats_all_reconnects_gives_zero_samples() {
        let mut agg = StatsAggregator::new();
        agg.add(make_record(1000, true));
        agg.add(make_record(2000, true));
        let report = agg.compute();
        assert_eq!(report.sample_count, 0);
        assert_eq!(report.reconnect_count, 2);
    }
}
