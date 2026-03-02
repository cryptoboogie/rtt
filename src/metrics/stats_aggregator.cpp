#include "metrics/stats_aggregator.h"
#include <algorithm>

namespace rtt {

void StatsAggregator::add(const TimestampRecord& rec) {
    if (rec.is_reconnect) {
        ++reconnect_count_;
        return;
    }
    warm_records_.push_back(rec);
}

PercentileSet StatsAggregator::compute_percentiles(std::vector<uint64_t>& values) {
    if (values.empty()) {
        return {};
    }

    std::sort(values.begin(), values.end());
    size_t n = values.size();

    auto percentile_index = [n](double p) -> size_t {
        size_t idx = static_cast<size_t>(p * static_cast<double>(n - 1));
        return std::min(idx, n - 1);
    };

    PercentileSet ps;
    ps.p50  = values[percentile_index(0.50)];
    ps.p95  = values[percentile_index(0.95)];
    ps.p99  = values[percentile_index(0.99)];
    ps.p999 = values[percentile_index(0.999)];
    ps.max  = values.back();
    return ps;
}

StatsReport StatsAggregator::compute() const {
    StatsReport report;
    report.reconnect_count = reconnect_count_;
    report.sample_count = warm_records_.size();

    if (warm_records_.empty()) {
        return report;
    }

    // Extract each derived metric into a separate vector
    std::vector<uint64_t> queue_delay_vals;
    std::vector<uint64_t> prep_time_vals;
    std::vector<uint64_t> trigger_to_wire_vals;
    std::vector<uint64_t> write_duration_vals;
    std::vector<uint64_t> write_to_first_byte_vals;
    std::vector<uint64_t> warm_ttfb_vals;
    std::vector<uint64_t> trigger_to_first_byte_vals;

    size_t n = warm_records_.size();
    queue_delay_vals.reserve(n);
    prep_time_vals.reserve(n);
    trigger_to_wire_vals.reserve(n);
    write_duration_vals.reserve(n);
    write_to_first_byte_vals.reserve(n);
    warm_ttfb_vals.reserve(n);
    trigger_to_first_byte_vals.reserve(n);

    for (const auto& rec : warm_records_) {
        queue_delay_vals.push_back(rec.queue_delay());
        prep_time_vals.push_back(rec.prep_time());
        trigger_to_wire_vals.push_back(rec.trigger_to_wire());
        write_duration_vals.push_back(rec.write_duration());
        write_to_first_byte_vals.push_back(rec.write_to_first_byte());
        warm_ttfb_vals.push_back(rec.warm_ttfb());
        trigger_to_first_byte_vals.push_back(rec.trigger_to_first_byte());
    }

    report.queue_delay          = compute_percentiles(queue_delay_vals);
    report.prep_time            = compute_percentiles(prep_time_vals);
    report.trigger_to_wire      = compute_percentiles(trigger_to_wire_vals);
    report.write_duration       = compute_percentiles(write_duration_vals);
    report.write_to_first_byte  = compute_percentiles(write_to_first_byte_vals);
    report.warm_ttfb            = compute_percentiles(warm_ttfb_vals);
    report.trigger_to_first_byte = compute_percentiles(trigger_to_first_byte_vals);

    return report;
}

} // namespace rtt
