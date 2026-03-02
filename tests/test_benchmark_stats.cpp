#include <gtest/gtest.h>
#include "benchmark/benchmark_runner.h"
#include "metrics/stats_aggregator.h"

using namespace rtt;

TEST(BenchmarkStats, AllTimestampsPopulatedInBenchmark) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_EQ(records.size(), 3u);

    for (size_t i = 0; i < records.size(); ++i) {
        const auto& rec = records[i];
        EXPECT_GT(rec.t_trigger_rx, 0u) << "Sample " << i;
        EXPECT_GT(rec.t_exec_start, 0u) << "Sample " << i;
        EXPECT_GT(rec.t_write_begin, 0u) << "Sample " << i;
        EXPECT_GT(rec.t_write_end, 0u) << "Sample " << i;
        EXPECT_GT(rec.t_first_resp_byte, 0u) << "Sample " << i;
        EXPECT_GT(rec.t_headers_done, 0u) << "Sample " << i;
    }
}

TEST(BenchmarkStats, DerivedMetricsAreReasonable) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    for (const auto& rec : records) {
        if (rec.is_reconnect) continue;

        // trigger_to_wire should be under 10ms on warm path
        EXPECT_LT(rec.trigger_to_wire(), 10'000'000u)
            << "trigger_to_wire too high: " << rec.trigger_to_wire() / 1000 << " us";

        // warm_ttfb should be reasonable (under 5 seconds)
        EXPECT_LT(rec.warm_ttfb(), 5'000'000'000ull)
            << "warm_ttfb unreasonable";

        // Timestamps should be monotonically ordered
        EXPECT_LE(rec.t_trigger_rx, rec.t_exec_start);
        EXPECT_LE(rec.t_exec_start, rec.t_write_begin);
        EXPECT_LE(rec.t_write_begin, rec.t_write_end);
        EXPECT_LE(rec.t_write_end, rec.t_first_resp_byte);
    }
}

TEST(BenchmarkStats, PercentilesComputedFromBenchmark) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::RandomCadence;
    config.sample_count = 5;
    config.pool_size = 1;
    config.min_interval_ms = 50;
    config.max_interval_ms = 100;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    StatsAggregator stats;
    for (const auto& rec : records) {
        stats.add(rec);
    }

    auto report = stats.compute();
    EXPECT_GT(report.sample_count, 0u);

    // All percentile fields should be non-zero for warm samples
    EXPECT_GT(report.trigger_to_wire.p50, 0u);
    EXPECT_GT(report.warm_ttfb.p50, 0u);
    EXPECT_GT(report.trigger_to_first_byte.p50, 0u);

    // p50 <= p95 <= p99 <= p99.9 <= max
    EXPECT_LE(report.trigger_to_wire.p50, report.trigger_to_wire.p95);
    EXPECT_LE(report.trigger_to_wire.p95, report.trigger_to_wire.p99);
    EXPECT_LE(report.trigger_to_wire.p99, report.trigger_to_wire.p999);
    EXPECT_LE(report.trigger_to_wire.p999, report.trigger_to_wire.max);
}

TEST(BenchmarkStats, BurstModeHasAllTimestamps) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::BurstRace;
    config.sample_count = 4;
    config.pool_size = 1;
    config.burst_size = 2;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_EQ(records.size(), 4u);

    StatsAggregator stats;
    for (const auto& rec : records) {
        stats.add(rec);
        EXPECT_GT(rec.t_trigger_rx, 0u);
        EXPECT_GT(rec.t_first_resp_byte, 0u);
    }

    auto report = stats.compute();
    EXPECT_GT(report.sample_count, 0u);
    EXPECT_GT(report.trigger_to_first_byte.p50, 0u);
}
