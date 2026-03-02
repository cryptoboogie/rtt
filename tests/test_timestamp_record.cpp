#include <gtest/gtest.h>
#include "metrics/timestamp_record.h"

using namespace rtt;

TEST(TimestampRecord, ZeroInitialized) {
    TimestampRecord rec{};
    EXPECT_EQ(rec.t_trigger_rx, 0u);
    EXPECT_EQ(rec.t_dispatch_q, 0u);
    EXPECT_EQ(rec.t_exec_start, 0u);
    EXPECT_EQ(rec.t_buf_ready, 0u);
    EXPECT_EQ(rec.t_write_begin, 0u);
    EXPECT_EQ(rec.t_write_end, 0u);
    EXPECT_EQ(rec.t_first_resp_byte, 0u);
    EXPECT_EQ(rec.t_headers_done, 0u);
    EXPECT_FALSE(rec.is_reconnect);
}

TEST(TimestampRecord, DerivedQueueDelay) {
    TimestampRecord rec{};
    rec.t_trigger_rx = 1000;
    rec.t_exec_start = 1500;
    EXPECT_EQ(rec.queue_delay(), 500u);
}

TEST(TimestampRecord, DerivedPrepTime) {
    TimestampRecord rec{};
    rec.t_exec_start = 2000;
    rec.t_buf_ready = 2100;
    EXPECT_EQ(rec.prep_time(), 100u);
}

TEST(TimestampRecord, DerivedTriggerToWire) {
    TimestampRecord rec{};
    rec.t_trigger_rx = 1000;
    rec.t_write_begin = 1200;
    EXPECT_EQ(rec.trigger_to_wire(), 200u);
}

TEST(TimestampRecord, DerivedWriteDuration) {
    TimestampRecord rec{};
    rec.t_write_begin = 3000;
    rec.t_write_end = 3050;
    EXPECT_EQ(rec.write_duration(), 50u);
}

TEST(TimestampRecord, DerivedWriteToFirstByte) {
    TimestampRecord rec{};
    rec.t_write_end = 4000;
    rec.t_first_resp_byte = 4800;
    EXPECT_EQ(rec.write_to_first_byte(), 800u);
}

TEST(TimestampRecord, DerivedWarmTtfb) {
    TimestampRecord rec{};
    rec.t_write_begin = 3000;
    rec.t_first_resp_byte = 3900;
    EXPECT_EQ(rec.warm_ttfb(), 900u);
}

TEST(TimestampRecord, DerivedTriggerToFirstByte) {
    TimestampRecord rec{};
    rec.t_trigger_rx = 1000;
    rec.t_first_resp_byte = 2500;
    EXPECT_EQ(rec.trigger_to_first_byte(), 1500u);
}

TEST(TimestampRecord, AllDerivedMetricsConsistent) {
    TimestampRecord rec{};
    rec.t_trigger_rx      = 100'000;
    rec.t_dispatch_q      = 100'050;
    rec.t_exec_start      = 100'100;
    rec.t_buf_ready       = 100'150;
    rec.t_write_begin     = 100'200;
    rec.t_write_end       = 100'250;
    rec.t_first_resp_byte = 200'000;
    rec.t_headers_done    = 200'100;

    EXPECT_EQ(rec.queue_delay(), 100u);
    EXPECT_EQ(rec.prep_time(), 50u);
    EXPECT_EQ(rec.trigger_to_wire(), 200u);
    EXPECT_EQ(rec.write_duration(), 50u);
    EXPECT_EQ(rec.write_to_first_byte(), 99'750u);
    EXPECT_EQ(rec.warm_ttfb(), 99'800u);
    EXPECT_EQ(rec.trigger_to_first_byte(), 100'000u);
}
