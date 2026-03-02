#include <gtest/gtest.h>
#include "connection/connection_pool.h"
#include "executor/ingress_thread.h"
#include "executor/execution_thread.h"
#include "executor/cpu_pin.h"
#include "request/request_template.h"
#include "trigger/trigger_message.h"

using namespace rtt;

TEST(IntegrationPipeline, EndToEndTriggerToResponse) {
    ConnectionPool pool("clob.polymarket.com", 443, 1);
    ASSERT_GE(pool.warmup(), 1u);

    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "clob.polymarket.com", "");

    TriggerQueue queue;
    IngressThread ingress(queue);
    ExecutionThread executor(queue, pool, tmpl);

    executor.start();
    ingress.inject(TriggerMessage::create(42));

    for (int i = 0; i < 100; ++i) {
        if (executor.processed_count() >= 1) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    executor.stop();

    auto records = executor.get_records();
    ASSERT_GE(records.size(), 1u);

    auto& rec = records[0];
    EXPECT_FALSE(rec.is_reconnect);
    EXPECT_GT(strlen(rec.cf_ray_pop), 0u) << "POP not extracted";
}

TEST(IntegrationPipeline, TimestampRecordComplete) {
    ConnectionPool pool("clob.polymarket.com", 443, 1);
    ASSERT_GE(pool.warmup(), 1u);

    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "clob.polymarket.com", "");

    TriggerQueue queue;
    IngressThread ingress(queue);
    ExecutionThread executor(queue, pool, tmpl);

    executor.start();
    ingress.inject(TriggerMessage::create(99));

    for (int i = 0; i < 100; ++i) {
        if (executor.processed_count() >= 1) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    executor.stop();

    auto records = executor.get_records();
    ASSERT_GE(records.size(), 1u);

    auto& rec = records[0];
    // All timestamps should be populated
    EXPECT_GT(rec.t_trigger_rx, 0u);
    EXPECT_GT(rec.t_exec_start, 0u);
    EXPECT_GT(rec.t_write_begin, 0u);
    EXPECT_GT(rec.t_first_resp_byte, 0u);

    // Derived metrics should be reasonable
    // trigger_to_wire should be under 10ms locally (just template patching + queue)
    EXPECT_LT(rec.trigger_to_wire(), 10'000'000u); // < 10ms
}

TEST(CpuPin, PinToCore) {
#ifdef __linux__
    EXPECT_TRUE(pin_to_core(0));
#else
    // macOS: should return false (no-op)
    EXPECT_FALSE(pin_to_core(0));
#endif
}
