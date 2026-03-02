#include <gtest/gtest.h>
#include "benchmark/benchmark_runner.h"

using namespace rtt;

TEST(BenchmarkRunner, SetupSucceeds) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 1;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());
}

TEST(BenchmarkRunner, SingleShotProducesRecords) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 2;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_EQ(records.size(), 2u);

    for (const auto& rec : records) {
        EXPECT_GT(rec.t_trigger_rx, 0u);
        EXPECT_GT(rec.t_first_resp_byte, 0u);
    }
}

TEST(BenchmarkRunner, RandomCadenceProducesRecords) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::RandomCadence;
    config.sample_count = 3;
    config.pool_size = 1;
    config.min_interval_ms = 50;
    config.max_interval_ms = 100;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_EQ(records.size(), 3u);
}

TEST(BenchmarkRunner, BurstRaceProducesRecords) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::BurstRace;
    config.sample_count = 4;
    config.pool_size = 1;
    config.burst_size = 2;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_EQ(records.size(), 4u);
}

TEST(BenchmarkRunner, LastPopExtracted) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 1;
    config.pool_size = 1;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    runner.run();
    std::string pop = runner.last_pop();
    EXPECT_FALSE(pop.empty()) << "POP should be extracted from cf-ray";
}
