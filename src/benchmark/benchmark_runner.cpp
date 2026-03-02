#include "benchmark/benchmark_runner.h"
#include "executor/cpu_pin.h"
#include "clock/monotonic_clock.h"
#include <random>
#include <chrono>
#include <thread>
#include <cstdio>

namespace rtt {

BenchmarkRunner::BenchmarkRunner(const BenchmarkConfig& config)
    : config_(config) {}

BenchmarkRunner::~BenchmarkRunner() {
    delete pool_;
}

bool BenchmarkRunner::setup() {
    std::printf("[bench] Warming %zu connections...\n", config_.pool_size);
    pool_ = new ConnectionPool("clob.polymarket.com", 443, config_.pool_size);
    size_t warmed = pool_->warmup();
    std::printf("[bench] Warmed %zu / %zu connections\n", warmed, config_.pool_size);
    if (warmed == 0) return false;

    template_.prepare("GET", "/", "clob.polymarket.com", "");

    // Send one warmup request to confirm the connection is truly ready
    // (ensures H2 SETTINGS exchange is complete)
    TriggerQueue warmup_queue;
    ExecutionThread warmup_exec(warmup_queue, *pool_, template_);
    TriggerMessage warmup_msg = TriggerMessage::create(0);
    warmup_msg.t_trigger_rx = MonotonicClock::now();
    warmup_queue.push(warmup_msg);
    auto rec = warmup_exec.process_one(warmup_msg);
    if (rec.t_first_resp_byte == 0) {
        std::fprintf(stderr, "[bench] Warmup request failed\n");
        return false;
    }
    std::printf("[bench] Warmup request OK, POP: %s\n", rec.cf_ray_pop);
    return true;
}

std::vector<TimestampRecord> BenchmarkRunner::run() {
    if (!pool_) return {};

    if (config_.pin_core >= 0) {
        bool pinned = pin_to_core(static_cast<size_t>(config_.pin_core));
        std::printf("[bench] CPU pin to core %d: %s\n",
                    config_.pin_core, pinned ? "ok" : "unavailable");
    }

    TriggerQueue queue;
    IngressThread ingress(queue);
    ExecutionThread executor(queue, *pool_, template_);

    executor.start();

    switch (config_.mode) {
        case BenchmarkMode::SingleShot:
            std::printf("[bench] Mode: single-shot, %zu samples\n", config_.sample_count);
            inject_single_shot(ingress, executor);
            break;
        case BenchmarkMode::RandomCadence:
            std::printf("[bench] Mode: random-cadence, %zu samples, interval %u-%ums\n",
                        config_.sample_count, config_.min_interval_ms, config_.max_interval_ms);
            inject_random_cadence(ingress, executor);
            break;
        case BenchmarkMode::BurstRace:
            std::printf("[bench] Mode: burst-race, %zu samples, burst size %zu\n",
                        config_.sample_count, config_.burst_size);
            inject_burst_race(ingress, executor);
            break;
    }

    wait_for_completion(executor, config_.sample_count);
    executor.stop();

    return executor.get_records();
}

std::string BenchmarkRunner::last_pop() const {
    if (!pool_) return "";
    return pool_->last_pop();
}

void BenchmarkRunner::inject_single_shot(IngressThread& ingress,
                                          ExecutionThread& executor) {
    // One trigger at a time, wait for completion between each
    for (size_t i = 0; i < config_.sample_count; ++i) {
        ingress.inject(TriggerMessage::create(static_cast<uint64_t>(i + 1)));

        // Wait for this trigger to complete before injecting next
        size_t expected = i + 1;
        for (int w = 0; w < 200; ++w) {
            if (executor.processed_count() >= expected) break;
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
        }

        // Brief idle between shots (200ms)
        std::this_thread::sleep_for(std::chrono::milliseconds(200));
    }
}

void BenchmarkRunner::inject_random_cadence(IngressThread& ingress,
                                             ExecutionThread& /*executor*/) {
    std::mt19937 rng(42); // Fixed seed for reproducibility
    std::uniform_int_distribution<uint32_t> dist(
        config_.min_interval_ms, config_.max_interval_ms);

    for (size_t i = 0; i < config_.sample_count; ++i) {
        ingress.inject(TriggerMessage::create(static_cast<uint64_t>(i + 1)));
        uint32_t delay_ms = dist(rng);
        std::this_thread::sleep_for(std::chrono::milliseconds(delay_ms));
    }
}

void BenchmarkRunner::inject_burst_race(IngressThread& ingress,
                                         ExecutionThread& executor) {
    size_t injected = 0;
    size_t burst_num = 0;

    while (injected < config_.sample_count) {
        size_t this_burst = std::min(config_.burst_size,
                                      config_.sample_count - injected);

        // Inject burst with minimal delay between triggers
        for (size_t j = 0; j < this_burst; ++j) {
            ingress.inject(TriggerMessage::create(
                static_cast<uint64_t>(injected + j + 1)));
        }
        injected += this_burst;
        ++burst_num;

        // Wait for burst to complete
        for (int w = 0; w < 200; ++w) {
            if (executor.processed_count() >= injected) break;
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
        }

        // Inter-burst pause (500ms)
        if (injected < config_.sample_count) {
            std::this_thread::sleep_for(std::chrono::milliseconds(500));
        }
    }
}

void BenchmarkRunner::wait_for_completion(ExecutionThread& executor,
                                           size_t expected) {
    for (int w = 0; w < 600; ++w) { // Up to 30 seconds
        if (executor.processed_count() >= expected) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
}

} // namespace rtt
