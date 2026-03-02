#include <gtest/gtest.h>
#include "metrics/stats_aggregator.h"

using namespace rtt;

static TimestampRecord make_record(uint64_t trigger_to_wire_ns, bool reconnect = false) {
    TimestampRecord rec{};
    rec.t_trigger_rx      = 1000;
    rec.t_dispatch_q      = 1000;
    rec.t_exec_start      = 1000 + trigger_to_wire_ns / 4;
    rec.t_buf_ready       = 1000 + trigger_to_wire_ns / 2;
    rec.t_write_begin     = 1000 + trigger_to_wire_ns;
    rec.t_write_end       = 1000 + trigger_to_wire_ns + 100;
    rec.t_first_resp_byte = 1000 + trigger_to_wire_ns + 1000;
    rec.t_headers_done    = 1000 + trigger_to_wire_ns + 1100;
    rec.is_reconnect      = reconnect;
    return rec;
}

TEST(StatsAggregator, EmptyReturnsZero) {
    StatsAggregator agg;
    auto report = agg.compute();
    EXPECT_EQ(report.sample_count, 0u);
    EXPECT_EQ(report.trigger_to_wire.p50, 0u);
    EXPECT_EQ(report.trigger_to_wire.p99, 0u);
}

TEST(StatsAggregator, SingleSample) {
    StatsAggregator agg;
    agg.add(make_record(500));
    auto report = agg.compute();
    EXPECT_EQ(report.sample_count, 1u);
    EXPECT_EQ(report.trigger_to_wire.p50, 500u);
    EXPECT_EQ(report.trigger_to_wire.p99, 500u);
    EXPECT_EQ(report.trigger_to_wire.p999, 500u);
    EXPECT_EQ(report.trigger_to_wire.max, 500u);
}

TEST(StatsAggregator, KnownDistribution) {
    StatsAggregator agg;
    for (uint64_t i = 1; i <= 100; ++i) {
        agg.add(make_record(i * 100));
    }
    auto report = agg.compute();
    EXPECT_EQ(report.sample_count, 100u);

    // p50 of [100..10000 step 100] → value at index 49 = 5000
    EXPECT_EQ(report.trigger_to_wire.p50, 5000u);
    // p95 → index 94 = 9500
    EXPECT_EQ(report.trigger_to_wire.p95, 9500u);
    // p99 → index 98 = 9900
    EXPECT_EQ(report.trigger_to_wire.p99, 9900u);
    // max = 10000
    EXPECT_EQ(report.trigger_to_wire.max, 10000u);
}

TEST(StatsAggregator, ReconnectFiltering) {
    StatsAggregator agg;
    agg.add(make_record(100, false));
    agg.add(make_record(200, false));
    agg.add(make_record(999999, true));  // reconnect — should be excluded from warm stats

    auto report = agg.compute();
    EXPECT_EQ(report.sample_count, 2u);
    EXPECT_EQ(report.reconnect_count, 1u);
    // Warm stats should only reflect the two warm samples
    EXPECT_LE(report.trigger_to_wire.max, 200u);
}

TEST(StatsAggregator, AllMetricsPresent) {
    StatsAggregator agg;
    for (uint64_t i = 1; i <= 10; ++i) {
        agg.add(make_record(i * 100));
    }
    auto report = agg.compute();

    // All metric groups should have non-zero values
    EXPECT_GT(report.trigger_to_wire.p50, 0u);
    EXPECT_GT(report.write_to_first_byte.p50, 0u);
    EXPECT_GT(report.trigger_to_first_byte.p50, 0u);
    EXPECT_GT(report.warm_ttfb.p50, 0u);
    EXPECT_GT(report.queue_delay.p50, 0u);
    EXPECT_GT(report.prep_time.p50, 0u);
    EXPECT_GT(report.write_duration.p50, 0u);
}
