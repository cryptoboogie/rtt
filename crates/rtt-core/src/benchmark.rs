use std::sync::Arc;
use std::time::Duration;

use crate::connection::{AddressFamily, ConnectionPool, extract_pop, get_cf_ray};
use crate::executor::{ExecutionThread, IngressThread, MaintenanceThread};
use crate::metrics::{StatsAggregator, StatsReport, TimestampRecord};
use crate::queue::TriggerQueue;
use crate::request::RequestTemplate;
use crate::trigger::{OrderType, Side, TriggerMessage};

/// Benchmark injection modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkMode {
    /// One trigger, wait, repeat.
    SingleShot,
    /// Random intervals between 50-500ms.
    RandomCadence,
    /// Bursts of N triggers separated by pauses.
    BurstRace,
}

/// Benchmark configuration.
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    pub host: String,
    pub port: u16,
    pub mode: BenchmarkMode,
    pub sample_count: usize,
    pub pool_size: usize,
    pub burst_size: usize,
    pub min_interval_ms: u32,
    pub max_interval_ms: u32,
    pub pin_core: Option<usize>,
    pub address_family: AddressFamily,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            host: "clob.polymarket.com".to_string(),
            port: 443,
            mode: BenchmarkMode::SingleShot,
            sample_count: 100,
            pool_size: 2,
            burst_size: 5,
            min_interval_ms: 50,
            max_interval_ms: 500,
            pin_core: None,
            // IPv6 default: tighter tail latency (p99 ~178ms vs IPv4 ~410ms from NYC)
            address_family: AddressFamily::V6,
        }
    }
}

/// Benchmark result with collected records and computed stats.
pub struct BenchmarkResult {
    pub records: Vec<TimestampRecord>,
    pub report: StatsReport,
    pub pop_distribution: Vec<(String, usize)>,
}

/// Run a benchmark with the given configuration.
pub async fn run_benchmark(
    config: &BenchmarkConfig,
) -> Result<BenchmarkResult, Box<dyn std::error::Error + Send + Sync>> {
    // 1. Create and warm connection pool
    let mut pool = ConnectionPool::new(
        &config.host,
        config.port,
        config.pool_size,
        config.address_family,
    );
    let warmed = pool.warmup().await?;
    eprintln!("Warmed {} connections to {}", warmed, config.host);

    let pool = Arc::new(pool);

    // 2. Create request template
    let mut template = RequestTemplate::new(http::Method::GET, "/".parse().unwrap());
    template.add_header("host", &config.host);

    // 3. Send one warmup request
    let warmup_req = template.build_request();
    let (warmup_resp, _) = pool.send(warmup_req).await?;
    if let Some(cf_ray) = get_cf_ray(&warmup_resp) {
        let pop = extract_pop(&cf_ray);
        eprintln!("POP: {}", pop);
    }

    // 4. Setup queue and threads
    let q = TriggerQueue::new();
    let ingress = IngressThread::new(q.sender());
    let mut exec = ExecutionThread::new(q.receiver());
    exec.start(pool.clone(), template);

    // 5. Optional CPU pin
    if let Some(core) = config.pin_core {
        crate::executor::pin_to_core(core);
    }

    // 6. Start maintenance thread
    let mut maintenance = MaintenanceThread::new();
    maintenance.start(pool.clone(), Duration::from_secs(5));

    // 7. Run injection based on mode
    let mut trigger_id = 0u64;
    match config.mode {
        BenchmarkMode::SingleShot => {
            for _ in 0..config.sample_count {
                let msg = make_trigger(trigger_id);
                trigger_id += 1;
                ingress.inject(msg).map_err(|e| e)?;
                // Wait for processing
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        BenchmarkMode::RandomCadence => {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            for _ in 0..config.sample_count {
                let msg = make_trigger(trigger_id);
                trigger_id += 1;
                ingress.inject(msg).map_err(|e| e)?;
                let delay = rng.gen_range(config.min_interval_ms..=config.max_interval_ms);
                tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            }
        }
        BenchmarkMode::BurstRace => {
            let num_bursts = (config.sample_count + config.burst_size - 1) / config.burst_size;
            let mut sent = 0;
            for _ in 0..num_bursts {
                let burst = config.burst_size.min(config.sample_count - sent);
                for _ in 0..burst {
                    let msg = make_trigger(trigger_id);
                    trigger_id += 1;
                    ingress.inject(msg).map_err(|e| e)?;
                    sent += 1;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    // 8. Wait for all records to be collected
    let expected = trigger_id as usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let count = exec.get_records().len();
        if count >= expected {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            eprintln!(
                "Warning: only got {}/{} records before timeout",
                count, expected
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 9. Stop threads
    exec.stop();
    maintenance.stop();

    // 10. Compute stats
    let records = exec.get_records();
    let mut agg = StatsAggregator::new();
    for rec in &records {
        agg.add(rec.clone());
    }
    let report = agg.compute();

    // POP distribution
    let mut pop_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for rec in &records {
        if !rec.cf_ray_pop.is_empty() {
            *pop_map.entry(rec.cf_ray_pop.clone()).or_insert(0) += 1;
        }
    }
    let mut pop_distribution: Vec<(String, usize)> = pop_map.into_iter().collect();
    pop_distribution.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(BenchmarkResult {
        records,
        report,
        pop_distribution,
    })
}

/// Print a benchmark report.
pub fn print_report(result: &BenchmarkResult) {
    let r = &result.report;
    println!("\n=== Benchmark Results ===");
    println!(
        "Samples: {} warm, {} reconnect",
        r.sample_count, r.reconnect_count
    );

    println!("\n{:<25} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "Metric", "p50", "p95", "p99", "p99.9", "max");
    println!("{}", "-".repeat(85));

    let print_row = |name: &str, ps: &crate::metrics::PercentileSet| {
        println!(
            "{:<25} {:>10} {:>10} {:>10} {:>10} {:>10}",
            name,
            format_ns(ps.p50),
            format_ns(ps.p95),
            format_ns(ps.p99),
            format_ns(ps.p999),
            format_ns(ps.max),
        );
    };

    print_row("queue_delay", &r.queue_delay);
    print_row("prep_time", &r.prep_time);
    print_row("trigger_to_wire", &r.trigger_to_wire);
    print_row("write_duration", &r.write_duration);
    print_row("write_to_first_byte", &r.write_to_first_byte);
    print_row("warm_ttfb", &r.warm_ttfb);
    print_row("trigger_to_first_byte", &r.trigger_to_first_byte);

    if !result.pop_distribution.is_empty() {
        println!("\nPOP Distribution:");
        for (pop, count) in &result.pop_distribution {
            println!("  {}: {} samples", pop, count);
        }
    }

    // Print per-record detail if small sample set
    if result.records.len() <= 10 {
        println!("\nPer-record detail:");
        for (i, rec) in result.records.iter().enumerate() {
            println!(
                "  [{}] ttw={} ttfb={} pop={} reconnect={}",
                i,
                format_ns(rec.trigger_to_wire()),
                format_ns(rec.trigger_to_first_byte()),
                rec.cf_ray_pop,
                rec.is_reconnect,
            );
        }
    }
}

fn format_ns(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.2}us", ns as f64 / 1_000.0)
    } else {
        format!("{}ns", ns)
    }
}

fn make_trigger(id: u64) -> TriggerMessage {
    TriggerMessage {
        trigger_id: id,
        token_id: "benchmark".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(),
        size: "1".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ns_units() {
        assert_eq!(format_ns(500), "500ns");
        assert_eq!(format_ns(5_000), "5.00us");
        assert_eq!(format_ns(5_000_000), "5.00ms");
    }

    #[test]
    fn default_config() {
        let cfg = BenchmarkConfig::default();
        assert_eq!(cfg.mode, BenchmarkMode::SingleShot);
        assert_eq!(cfg.sample_count, 100);
        assert_eq!(cfg.pool_size, 2);
    }

    #[tokio::test]
    async fn single_shot_benchmark() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 3,
            pool_size: 1,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        assert_eq!(result.records.len(), 3);
        assert_eq!(result.report.sample_count + result.report.reconnect_count, 3);
    }

    #[tokio::test]
    async fn random_cadence_benchmark() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::RandomCadence,
            sample_count: 3,
            pool_size: 1,
            min_interval_ms: 50,
            max_interval_ms: 100,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        assert_eq!(result.records.len(), 3);
    }

    #[tokio::test]
    async fn burst_race_benchmark() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::BurstRace,
            sample_count: 6,
            pool_size: 1,
            burst_size: 3,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        assert_eq!(result.records.len(), 6);
    }

    #[tokio::test]
    async fn benchmark_timestamps_populated() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 1,
            pool_size: 1,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        let rec = &result.records[0];
        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_exec_start > 0);
        assert!(rec.t_write_begin > 0);
        assert!(rec.t_write_end > 0);
        assert!(rec.t_first_resp_byte > 0);
        assert!(rec.t_headers_done > 0);
    }

    #[tokio::test]
    async fn benchmark_pop_extracted() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 2,
            pool_size: 1,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        // At least one record should have POP
        let has_pop = result.records.iter().any(|r| !r.cf_ray_pop.is_empty());
        assert!(has_pop, "no POP extracted from any record");
        assert!(!result.pop_distribution.is_empty());
    }

    #[tokio::test]
    async fn benchmark_warm_cold_separation() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 3,
            pool_size: 1,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("benchmark failed");
        // All samples should be warm (no reconnect on single-shot with warmed pool)
        assert_eq!(result.report.reconnect_count, 0);
        assert_eq!(result.report.sample_count, 3);
    }

    // === Protocol experiment tests ===

    #[tokio::test]
    async fn ipv4_forced_path() {
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 1,
            pool_size: 1,
            address_family: AddressFamily::V4,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("IPv4 benchmark failed");
        assert_eq!(result.records.len(), 1);
        assert!(!result.records[0].cf_ray_pop.is_empty());
    }

    #[tokio::test]
    async fn ipv6_forced_path() {
        // IPv6 may not be available, so we allow failure
        let config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 1,
            pool_size: 1,
            address_family: AddressFamily::V6,
            ..Default::default()
        };
        match run_benchmark(&config).await {
            Ok(result) => {
                assert_eq!(result.records.len(), 1);
            }
            Err(_) => {
                // IPv6 not available, that's OK
            }
        }
    }

    #[tokio::test]
    async fn dual_connection_comparison() {
        // Single connection
        let config_single = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 2,
            pool_size: 1,
            ..Default::default()
        };
        let result_single = run_benchmark(&config_single).await.expect("single conn failed");
        assert_eq!(result_single.records.len(), 2);

        // Dual connection
        let config_dual = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 2,
            pool_size: 2,
            ..Default::default()
        };
        let result_dual = run_benchmark(&config_dual).await.expect("dual conn failed");
        assert_eq!(result_dual.records.len(), 2);
    }

    #[tokio::test]
    async fn dual_connection_burst_contention() {
        // First, run single-shot as a baseline for latency comparison
        let baseline_config = BenchmarkConfig {
            mode: BenchmarkMode::SingleShot,
            sample_count: 2,
            pool_size: 2,
            ..Default::default()
        };
        let baseline = run_benchmark(&baseline_config).await.expect("baseline failed");

        // Now run burst with pool_size=2
        let config = BenchmarkConfig {
            mode: BenchmarkMode::BurstRace,
            sample_count: 4,
            pool_size: 2,
            burst_size: 2,
            ..Default::default()
        };
        let result = run_benchmark(&config).await.expect("burst benchmark failed");
        assert_eq!(result.records.len(), 4);

        // Assert distribution: both connection indices (0 and 1) must appear
        let indices: std::collections::HashSet<usize> =
            result.records.iter().map(|r| r.connection_index).collect();
        assert!(
            indices.contains(&0) && indices.contains(&1),
            "Expected both connections used, got indices: {:?}",
            indices,
        );

        // Assert no contention degradation: burst warm_ttfb (write_begin →
        // first_resp_byte, pure network time excluding queue delay) should not
        // be dramatically worse than baseline. A 3x blowup would indicate
        // connection-level contention or deadlock rather than normal queuing.
        let baseline_ttfb = baseline.report.warm_ttfb.p50;
        let burst_ttfb = result.report.warm_ttfb.p50;
        if baseline_ttfb > 0 {
            assert!(
                burst_ttfb < baseline_ttfb * 3,
                "Burst warm_ttfb p50 ({}) >= 3x baseline ({}) — connection contention detected",
                burst_ttfb,
                baseline_ttfb,
            );
        }
    }
}
