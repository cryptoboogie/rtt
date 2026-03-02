#pragma once

#include "metrics/timestamp_record.h"
#include <vector>
#include <cstdint>
#include <cstddef>

namespace rtt {

struct PercentileSet {
    uint64_t p50  = 0;
    uint64_t p95  = 0;
    uint64_t p99  = 0;
    uint64_t p999 = 0;
    uint64_t max  = 0;
};

struct StatsReport {
    size_t sample_count    = 0;
    size_t reconnect_count = 0;

    PercentileSet queue_delay;
    PercentileSet prep_time;
    PercentileSet trigger_to_wire;
    PercentileSet write_duration;
    PercentileSet write_to_first_byte;
    PercentileSet warm_ttfb;
    PercentileSet trigger_to_first_byte;
};

class StatsAggregator {
public:
    void add(const TimestampRecord& rec);
    StatsReport compute() const;

private:
    std::vector<TimestampRecord> warm_records_;
    size_t reconnect_count_ = 0;

    static PercentileSet compute_percentiles(std::vector<uint64_t>& values);
};

} // namespace rtt
