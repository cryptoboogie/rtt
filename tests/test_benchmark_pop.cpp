#include <gtest/gtest.h>
#include "benchmark/benchmark_runner.h"
#include "metrics/stats_aggregator.h"
#include <map>
#include <string>
#include <cstring>

using namespace rtt;

TEST(BenchmarkPOP, PopExtractedInEveryWarmRecord) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    for (size_t i = 0; i < records.size(); ++i) {
        const auto& rec = records[i];
        if (rec.is_reconnect) continue;
        EXPECT_GT(std::strlen(rec.cf_ray_pop), 0u)
            << "POP not extracted in warm sample " << i;
    }
}

TEST(BenchmarkPOP, PopConsistentAcrossSamples) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 3;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    // Build POP distribution
    std::map<std::string, size_t> pop_dist;
    for (const auto& rec : records) {
        if (rec.is_reconnect) continue;
        if (std::strlen(rec.cf_ray_pop) > 0) {
            pop_dist[rec.cf_ray_pop]++;
        }
    }

    // At least one POP should be observed
    EXPECT_FALSE(pop_dist.empty()) << "No POP extracted from any sample";

    // Print POP distribution for observability
    for (const auto& [pop, count] : pop_dist) {
        std::printf("  POP %s: %zu samples\n", pop.c_str(), count);
    }
}

TEST(BenchmarkPOP, WarmColdSeparation) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 5;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    size_t warm_count = 0;
    size_t cold_count = 0;

    for (const auto& rec : records) {
        if (rec.is_reconnect) {
            ++cold_count;
        } else {
            ++warm_count;
            // Warm samples must have valid timestamps
            EXPECT_GT(rec.t_write_begin, 0u);
            EXPECT_GT(rec.t_first_resp_byte, 0u);
        }
    }

    // After warmup, all samples should be warm (no reconnects expected)
    EXPECT_GT(warm_count, 0u);
    std::printf("  Warm: %zu, Cold/Reconnect: %zu\n", warm_count, cold_count);

    // StatsAggregator should filter reconnects correctly
    StatsAggregator stats;
    for (const auto& rec : records) {
        stats.add(rec);
    }
    auto report = stats.compute();
    EXPECT_EQ(report.sample_count, warm_count);
    EXPECT_EQ(report.reconnect_count, cold_count);
}

TEST(BenchmarkPOP, LastPopMatchesRunnerPop) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 2;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    std::string runner_pop = runner.last_pop();
    EXPECT_FALSE(runner_pop.empty());

    // At least one record's POP should match the pool's last_pop
    bool found = false;
    for (const auto& rec : records) {
        if (std::strlen(rec.cf_ray_pop) > 0) {
            // POP from pool includes the full suffix; record has just 3-letter code
            if (runner_pop.find(rec.cf_ray_pop) != std::string::npos ||
                std::string(rec.cf_ray_pop) == runner_pop) {
                found = true;
                break;
            }
        }
    }
    EXPECT_TRUE(found) << "Runner POP '" << runner_pop
                       << "' not found in any record";
}
