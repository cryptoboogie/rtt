#include <gtest/gtest.h>
#include "clock/monotonic_clock.h"
#include <thread>

using namespace rtt;

TEST(MonotonicClock, NowReturnsNonZero) {
    uint64_t t = MonotonicClock::now();
    EXPECT_GT(t, 0u);
}

TEST(MonotonicClock, IsMonotonic) {
    uint64_t t1 = MonotonicClock::now();
    uint64_t t2 = MonotonicClock::now();
    EXPECT_GE(t2, t1);
}

TEST(MonotonicClock, SubMillisecondResolution) {
    uint64_t t1 = MonotonicClock::now();
    // Tight loop — difference should be well under 1ms (1,000,000 ns)
    uint64_t t2 = MonotonicClock::now();
    uint64_t diff = t2 - t1;
    EXPECT_LT(diff, 1'000'000u) << "Two successive calls took >= 1ms, clock may be too coarse";
}

TEST(MonotonicClock, MeasuresRealTime) {
    uint64_t t1 = MonotonicClock::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
    uint64_t t2 = MonotonicClock::now();
    uint64_t diff_ns = t2 - t1;
    // Should be at least 5ms (allowing for sleep imprecision)
    EXPECT_GE(diff_ns, 5'000'000u);
    // Should be less than 100ms
    EXPECT_LT(diff_ns, 100'000'000u);
}
