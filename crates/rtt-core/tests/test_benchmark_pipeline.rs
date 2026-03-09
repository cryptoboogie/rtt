//! # Benchmark Pipeline Tests
//!
//! These tests keep the explicit live benchmark lane separate from `--lib`
//! while preserving the pre-existing coverage for benchmark modes and
//! connection experiments.

use rtt_core::benchmark::{run_benchmark, BenchmarkConfig, BenchmarkMode};
use rtt_core::connection::AddressFamily;

#[tokio::test]
async fn single_shot_benchmark_collects_expected_samples() {
    let config = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 3,
        pool_size: 1,
        ..Default::default()
    };
    let result = run_benchmark(&config).await.expect("benchmark failed");
    assert_eq!(result.records.len(), 3);
    assert_eq!(
        result.report.sample_count + result.report.reconnect_count,
        3
    );
}

#[tokio::test]
async fn random_cadence_benchmark_collects_expected_samples() {
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
async fn burst_race_benchmark_collects_expected_samples() {
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
async fn benchmark_records_populated_timestamps() {
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
async fn benchmark_extracts_pop_distribution() {
    let config = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 2,
        pool_size: 1,
        ..Default::default()
    };
    let result = run_benchmark(&config).await.expect("benchmark failed");
    assert!(
        result
            .records
            .iter()
            .any(|record| !record.cf_ray_pop.is_empty()),
        "no POP extracted from any sample"
    );
    assert!(!result.pop_distribution.is_empty());
}

#[tokio::test]
async fn benchmark_keeps_single_shot_samples_warm() {
    let config = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 3,
        pool_size: 1,
        ..Default::default()
    };
    let result = run_benchmark(&config).await.expect("benchmark failed");
    assert_eq!(result.report.reconnect_count, 0);
    assert_eq!(result.report.sample_count, 3);
}

#[tokio::test]
async fn benchmark_supports_forced_ipv4() {
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
async fn benchmark_allows_ipv6_probe() {
    let config = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 1,
        pool_size: 1,
        address_family: AddressFamily::V6,
        ..Default::default()
    };
    match run_benchmark(&config).await {
        Ok(result) => assert_eq!(result.records.len(), 1),
        Err(_) => {}
    }
}

#[tokio::test]
async fn benchmark_compares_single_and_dual_connection_modes() {
    let config_single = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 2,
        pool_size: 1,
        ..Default::default()
    };
    let result_single = run_benchmark(&config_single)
        .await
        .expect("single-connection benchmark failed");
    assert_eq!(result_single.records.len(), 2);

    let config_dual = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 2,
        pool_size: 2,
        ..Default::default()
    };
    let result_dual = run_benchmark(&config_dual)
        .await
        .expect("dual-connection benchmark failed");
    assert_eq!(result_dual.records.len(), 2);
}

#[tokio::test]
async fn dual_connection_burst_stays_within_reasonable_contention_bounds() {
    let baseline_config = BenchmarkConfig {
        mode: BenchmarkMode::SingleShot,
        sample_count: 2,
        pool_size: 2,
        ..Default::default()
    };
    let baseline = run_benchmark(&baseline_config)
        .await
        .expect("baseline benchmark failed");

    let config = BenchmarkConfig {
        mode: BenchmarkMode::BurstRace,
        sample_count: 4,
        pool_size: 2,
        burst_size: 2,
        ..Default::default()
    };
    let result = run_benchmark(&config)
        .await
        .expect("burst benchmark failed");
    assert_eq!(result.records.len(), 4);

    let indices: std::collections::HashSet<usize> = result
        .records
        .iter()
        .map(|record| record.connection_index)
        .collect();
    assert!(
        indices.contains(&0) && indices.contains(&1),
        "expected both connection indices to appear, got {:?}",
        indices
    );

    let baseline_ttfb = baseline.report.warm_ttfb.p50;
    let burst_ttfb = result.report.warm_ttfb.p50;
    if baseline_ttfb > 0 {
        assert!(
            burst_ttfb < baseline_ttfb * 3,
            "burst warm_ttfb p50 ({}) should stay below 3x baseline ({})",
            burst_ttfb,
            baseline_ttfb
        );
    }
}
