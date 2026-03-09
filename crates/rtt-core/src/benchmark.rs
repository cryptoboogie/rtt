use std::sync::Arc;
use std::time::Duration;

use crate::connection::{extract_pop, get_cf_ray, AddressFamily, ConnectionPool};
use crate::executor::{ExecutionThread, IngressThread, MaintenanceThread};
use crate::metrics::{StatsAggregator, StatsReport, TimestampRecord};
use crate::polymarket::{CLOB_HOST, CLOB_PORT};
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
            host: CLOB_HOST.to_string(),
            port: CLOB_PORT,
            mode: BenchmarkMode::SingleShot,
            sample_count: 100,
            pool_size: 2,
            burst_size: 5,
            min_interval_ms: 50,
            max_interval_ms: 500,
            pin_core: None,
            // Auto: use whatever address family the system provides.
            // V6 has tighter tail latency but not all environments support it.
            address_family: AddressFamily::Auto,
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

    println!(
        "\n{:<25} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "Metric", "p50", "p95", "p99", "p99.9", "max"
    );
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
}
