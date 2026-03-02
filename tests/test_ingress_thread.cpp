#include <gtest/gtest.h>
#include "executor/ingress_thread.h"

using namespace rtt;

TEST(IngressThread, EnqueuesTrigger) {
    TriggerQueue queue;
    IngressThread ingress(queue);

    auto msg = TriggerMessage::create(1);
    EXPECT_TRUE(ingress.inject(msg));
    EXPECT_FALSE(queue.empty());

    auto popped = queue.pop();
    ASSERT_TRUE(popped.has_value());
    EXPECT_EQ(popped->trigger_id, 1u);
}

TEST(IngressThread, SetsTimestamp) {
    TriggerQueue queue;
    IngressThread ingress(queue);

    auto msg = TriggerMessage::create(2);
    EXPECT_EQ(msg.t_trigger_rx, 0u); // Not set yet
    ingress.inject(msg);

    auto popped = queue.pop();
    ASSERT_TRUE(popped.has_value());
    EXPECT_GT(popped->t_trigger_rx, 0u); // Should be set by inject()
}

TEST(IngressThread, StartsAndStops) {
    TriggerQueue queue;
    IngressThread ingress(queue);

    ingress.start();
    EXPECT_TRUE(ingress.is_running());

    // Inject while running
    auto msg = TriggerMessage::create(3);
    EXPECT_TRUE(ingress.inject(msg));

    ingress.stop();
    EXPECT_FALSE(ingress.is_running());

    // Verify the trigger was enqueued
    auto popped = queue.pop();
    ASSERT_TRUE(popped.has_value());
    EXPECT_EQ(popped->trigger_id, 3u);
}

TEST(IngressThread, MultipleInjects) {
    TriggerQueue queue;
    IngressThread ingress(queue);

    for (uint64_t i = 0; i < 10; ++i) {
        EXPECT_TRUE(ingress.inject(TriggerMessage::create(i)));
    }

    for (uint64_t i = 0; i < 10; ++i) {
        auto popped = queue.pop();
        ASSERT_TRUE(popped.has_value());
        EXPECT_EQ(popped->trigger_id, i);
        EXPECT_GT(popped->t_trigger_rx, 0u);
    }
}
