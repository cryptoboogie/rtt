#pragma once

#include <cstdint>

namespace rtt {

// High-resolution monotonic clock returning nanoseconds.
// On Linux: uses CLOCK_MONOTONIC_RAW for best stability.
// On macOS: uses mach_absolute_time with timebase conversion.
// Returns nanoseconds since an arbitrary epoch.
class MonotonicClock {
public:
    static uint64_t now() noexcept;
};

} // namespace rtt
