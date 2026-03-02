#pragma once

#include "connection/connection_pool.h"
#include "executor/ingress_thread.h"
#include "executor/execution_thread.h"
#include "request/request_template.h"
#include "metrics/stats_aggregator.h"
#include <cstddef>
#include <cstdint>
#include <vector>

namespace rtt {

enum class BenchmarkMode {
    SingleShot,    // One trigger, long idle — best-case hot-path
    RandomCadence, // Randomized inter-arrival (50ms–500ms jittered)
    BurstRace      // Short bursts of 2–20 triggers close together
};

struct BenchmarkConfig {
    BenchmarkMode mode = BenchmarkMode::SingleShot;
    size_t sample_count = 100;   // Total triggers to inject
    size_t pool_size = 2;        // Number of warm connections
    size_t burst_size = 5;       // Triggers per burst (BurstRace mode)
    uint32_t min_interval_ms = 50;  // RandomCadence min inter-arrival
    uint32_t max_interval_ms = 500; // RandomCadence max inter-arrival
    int pin_core = -1;           // CPU core to pin executor (-1 = no pin)
    AddressFamily address_family = AddressFamily::AUTO; // Force v4/v6
};

class BenchmarkRunner {
public:
    explicit BenchmarkRunner(const BenchmarkConfig& config);
    ~BenchmarkRunner();

    // Non-copyable
    BenchmarkRunner(const BenchmarkRunner&) = delete;
    BenchmarkRunner& operator=(const BenchmarkRunner&) = delete;

    // Initialize connections and request template. Returns false on failure.
    bool setup();

    // Run the benchmark. Returns collected records.
    std::vector<TimestampRecord> run();

    // Get the POP observed during the run.
    std::string last_pop() const;

private:
    BenchmarkConfig config_;
    ConnectionPool* pool_ = nullptr;
    RequestTemplate template_;

    void inject_single_shot(IngressThread& ingress, ExecutionThread& executor);
    void inject_random_cadence(IngressThread& ingress, ExecutionThread& executor);
    void inject_burst_race(IngressThread& ingress, ExecutionThread& executor);

    void wait_for_completion(ExecutionThread& executor, size_t expected);
};

} // namespace rtt
