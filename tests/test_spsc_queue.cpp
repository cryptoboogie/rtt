#include <gtest/gtest.h>
#include "queue/spsc_queue.h"
#include <thread>
#include <vector>

using namespace rtt;

TEST(SPSCQueue, PushPopSingleElement) {
    SPSCQueue<int, 4> q;
    EXPECT_TRUE(q.push(42));
    auto val = q.pop();
    ASSERT_TRUE(val.has_value());
    EXPECT_EQ(*val, 42);
}

TEST(SPSCQueue, EmptyPopReturnsNullopt) {
    SPSCQueue<int, 4> q;
    EXPECT_FALSE(q.pop().has_value());
    EXPECT_TRUE(q.empty());
}

TEST(SPSCQueue, FullPushReturnsFalse) {
    // Capacity 4 means usable slots = 3 (one slot reserved to distinguish full from empty)
    SPSCQueue<int, 4> q;
    EXPECT_TRUE(q.push(1));
    EXPECT_TRUE(q.push(2));
    EXPECT_TRUE(q.push(3));
    EXPECT_FALSE(q.push(4)); // full
}

TEST(SPSCQueue, WrapAround) {
    SPSCQueue<int, 4> q;
    // Fill and drain multiple times to force wrap-around
    for (int round = 0; round < 10; ++round) {
        for (int i = 0; i < 3; ++i) {
            EXPECT_TRUE(q.push(round * 10 + i));
        }
        for (int i = 0; i < 3; ++i) {
            auto val = q.pop();
            ASSERT_TRUE(val.has_value());
            EXPECT_EQ(*val, round * 10 + i);
        }
    }
}

TEST(SPSCQueue, FIFOOrder) {
    SPSCQueue<int, 8> q;
    q.push(10);
    q.push(20);
    q.push(30);
    EXPECT_EQ(*q.pop(), 10);
    EXPECT_EQ(*q.pop(), 20);
    EXPECT_EQ(*q.pop(), 30);
}

TEST(SPSCQueue, ConcurrentProducerConsumer) {
    constexpr size_t N = 100'000;
    SPSCQueue<uint64_t, 1024> q;

    std::vector<uint64_t> received;
    received.reserve(N);

    std::thread producer([&] {
        for (uint64_t i = 0; i < N; ++i) {
            while (!q.push(i)) {
                // spin — queue is full
            }
        }
    });

    std::thread consumer([&] {
        for (size_t count = 0; count < N;) {
            auto val = q.pop();
            if (val.has_value()) {
                received.push_back(*val);
                ++count;
            }
        }
    });

    producer.join();
    consumer.join();

    ASSERT_EQ(received.size(), N);
    for (uint64_t i = 0; i < N; ++i) {
        EXPECT_EQ(received[i], i) << "Mismatch at index " << i;
    }
}
