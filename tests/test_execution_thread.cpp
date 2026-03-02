#include <gtest/gtest.h>
#include "executor/execution_thread.h"
#include "executor/ingress_thread.h"

using namespace rtt;

class ExecutionThreadTest : public ::testing::Test {
protected:
    ConnectionPool pool_{"clob.polymarket.com", 443, 1};
    TriggerQueue queue_;
    RequestTemplate template_;

    void SetUp() override {
        ASSERT_GE(pool_.warmup(), 1u) << "Need at least 1 warm connection";
        template_.prepare("GET", "/", "clob.polymarket.com", "");
    }
};

TEST_F(ExecutionThreadTest, ProcessOneReturnsRecord) {
    ExecutionThread exec(queue_, pool_, template_);

    TriggerMessage msg = TriggerMessage::create(1);
    msg.t_trigger_rx = MonotonicClock::now();

    auto rec = exec.process_one(msg);

    EXPECT_FALSE(rec.is_reconnect);
    EXPECT_GT(rec.t_trigger_rx, 0u);
    EXPECT_GT(rec.t_exec_start, 0u);
    EXPECT_GT(rec.t_buf_ready, 0u);
    EXPECT_GT(rec.t_write_begin, 0u);
    EXPECT_GT(rec.t_write_end, 0u);
    EXPECT_GT(rec.t_first_resp_byte, 0u);
    EXPECT_GT(rec.t_headers_done, 0u);
}

TEST_F(ExecutionThreadTest, TimestampsAreMonotonic) {
    ExecutionThread exec(queue_, pool_, template_);

    TriggerMessage msg = TriggerMessage::create(2);
    msg.t_trigger_rx = MonotonicClock::now();

    auto rec = exec.process_one(msg);

    EXPECT_LE(rec.t_trigger_rx, rec.t_exec_start);
    EXPECT_LE(rec.t_exec_start, rec.t_buf_ready);
    EXPECT_LE(rec.t_buf_ready, rec.t_write_begin);
    EXPECT_LE(rec.t_write_begin, rec.t_write_end);
    EXPECT_LE(rec.t_write_end, rec.t_first_resp_byte);
    EXPECT_LE(rec.t_first_resp_byte, rec.t_headers_done);
}

TEST_F(ExecutionThreadTest, CfRayPopExtracted) {
    ExecutionThread exec(queue_, pool_, template_);

    TriggerMessage msg = TriggerMessage::create(3);
    msg.t_trigger_rx = MonotonicClock::now();

    auto rec = exec.process_one(msg);

    // cf_ray_pop should have a 3-letter POP code
    EXPECT_GT(strlen(rec.cf_ray_pop), 0u) << "cf-ray POP not extracted";
}

TEST_F(ExecutionThreadTest, ThreadedProcessing) {
    IngressThread ingress(queue_);
    ExecutionThread exec(queue_, pool_, template_);

    exec.start();

    // Inject a trigger
    ingress.inject(TriggerMessage::create(10));

    // Wait for processing
    for (int i = 0; i < 100; ++i) {
        if (exec.processed_count() >= 1) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }

    exec.stop();

    EXPECT_GE(exec.processed_count(), 1u);
    auto records = exec.get_records();
    ASSERT_GE(records.size(), 1u);
    EXPECT_GT(records[0].t_trigger_rx, 0u);
    EXPECT_GT(records[0].trigger_to_wire(), 0u);
}
