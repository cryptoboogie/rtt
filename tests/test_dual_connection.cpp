#include <gtest/gtest.h>
#include "benchmark/benchmark_runner.h"
#include "metrics/stats_aggregator.h"
#include <cstdio>

using namespace rtt;

TEST(DualConnection, SingleConnectionBenchmark) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 1;

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

    std::printf("  Single connection - trigger_to_wire p50: %.1f us\n",
                static_cast<double>(report.trigger_to_wire.p50) / 1000.0);
    std::printf("  Single connection - warm_ttfb p50: %.1f us\n",
                static_cast<double>(report.warm_ttfb.p50) / 1000.0);
}

TEST(DualConnection, DualConnectionBenchmark) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 2;

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

    std::printf("  Dual connection - trigger_to_wire p50: %.1f us\n",
                static_cast<double>(report.trigger_to_wire.p50) / 1000.0);
    std::printf("  Dual connection - warm_ttfb p50: %.1f us\n",
                static_cast<double>(report.warm_ttfb.p50) / 1000.0);
}

TEST(DualConnection, BurstModeWithDualConnections) {
    // Burst mode is where dual connections matter most — contention under load
    BenchmarkConfig config;
    config.mode = BenchmarkMode::BurstRace;
    config.sample_count = 6;
    config.pool_size = 2;
    config.burst_size = 3;

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

    std::printf("  Burst dual - trigger_to_wire p50: %.1f us, p99: %.1f us\n",
                static_cast<double>(report.trigger_to_wire.p50) / 1000.0,
                static_cast<double>(report.trigger_to_wire.p99) / 1000.0);
}
