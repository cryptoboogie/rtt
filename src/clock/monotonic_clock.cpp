#include "clock/monotonic_clock.h"

#if defined(__APPLE__)
#include <mach/mach_time.h>
#elif defined(__linux__)
#include <time.h>
#else
#include <chrono>
#endif

namespace rtt {

#if defined(__APPLE__)

static mach_timebase_info_data_t get_timebase() {
    mach_timebase_info_data_t info;
    mach_timebase_info(&info);
    return info;
}

uint64_t MonotonicClock::now() noexcept {
    static const auto info = get_timebase();
    uint64_t ticks = mach_absolute_time();
    // Convert ticks to nanoseconds using timebase ratio
    return ticks * info.numer / info.denom;
}

#elif defined(__linux__)

uint64_t MonotonicClock::now() noexcept {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC_RAW, &ts);
    return static_cast<uint64_t>(ts.tv_sec) * 1'000'000'000ULL
         + static_cast<uint64_t>(ts.tv_nsec);
}

#else

uint64_t MonotonicClock::now() noexcept {
    auto tp = std::chrono::steady_clock::now();
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::nanoseconds>(
            tp.time_since_epoch()).count());
}

#endif

} // namespace rtt
