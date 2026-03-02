#include <cstdlib>
#include <cstdio>
#include <cstring>

#include "connection/connection_pool.h"
#include "executor/ingress_thread.h"
#include "executor/execution_thread.h"
#include "executor/maintenance_thread.h"
#include "executor/cpu_pin.h"
#include "request/request_template.h"
#include "trigger/trigger_message.h"
#include "clock/monotonic_clock.h"
#include "metrics/stats_aggregator.h"
#include "benchmark/benchmark_runner.h"

using namespace rtt;

static void print_record(const TimestampRecord& rec) {
    auto us = [](uint64_t ns) { return static_cast<double>(ns) / 1000.0; };

    std::printf("=== Timestamp Record ===\n");
    std::printf("  queue_delay:          %8.1f us\n", us(rec.queue_delay()));
    std::printf("  prep_time:            %8.1f us\n", us(rec.prep_time()));
    std::printf("  trigger_to_wire:      %8.1f us\n", us(rec.trigger_to_wire()));
    std::printf("  write_duration:       %8.1f us\n", us(rec.write_duration()));
    std::printf("  write_to_first_byte:  %8.1f us\n", us(rec.write_to_first_byte()));
    std::printf("  warm_ttfb:            %8.1f us\n", us(rec.warm_ttfb()));
    std::printf("  trigger_to_first_byte:%8.1f us\n", us(rec.trigger_to_first_byte()));
    std::printf("  cf_ray_pop:           %s\n", rec.cf_ray_pop);
    std::printf("  is_reconnect:         %s\n", rec.is_reconnect ? "yes" : "no");
}

static void print_percentile_set(const char* name, const PercentileSet& ps) {
    auto us = [](uint64_t ns) { return static_cast<double>(ns) / 1000.0; };
    std::printf("  %-24s p50=%8.1f  p95=%8.1f  p99=%8.1f  p99.9=%8.1f  max=%8.1f us\n",
                name, us(ps.p50), us(ps.p95), us(ps.p99), us(ps.p999), us(ps.max));
}

static void print_usage() {
    std::printf("Usage:\n");
    std::printf("  rtt-executor --trigger-test\n");
    std::printf("  rtt-executor --benchmark [options]\n");
    std::printf("\n");
    std::printf("Benchmark options:\n");
    std::printf("  --mode <single-shot|random-cadence|burst-race>  (default: single-shot)\n");
    std::printf("  --samples <N>           Number of triggers (default: 100)\n");
    std::printf("  --connections <N>       Warm connection pool size (default: 2)\n");
    std::printf("  --burst-size <N>        Triggers per burst in burst-race mode (default: 5)\n");
    std::printf("  --min-interval <ms>     Min inter-arrival for random-cadence (default: 50)\n");
    std::printf("  --max-interval <ms>     Max inter-arrival for random-cadence (default: 500)\n");
    std::printf("  --pin-core <N>          Pin executor to CPU core (default: no pin)\n");
}

static int run_trigger_test() {
    std::printf("Warming up connections...\n");
    ConnectionPool pool("clob.polymarket.com", 443, 2);
    size_t warmed = pool.warmup();
    std::printf("Warmed %zu connections\n", warmed);
    if (warmed == 0) {
        std::fprintf(stderr, "Failed to establish any connections\n");
        return EXIT_FAILURE;
    }

    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "clob.polymarket.com", "");

    TriggerQueue queue;
    IngressThread ingress(queue);
    ExecutionThread executor(queue, pool, tmpl);

    executor.start();

    std::printf("Injecting trigger...\n");
    ingress.inject(TriggerMessage::create(1));

    for (int i = 0; i < 100; ++i) {
        if (executor.processed_count() >= 1) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }

    executor.stop();

    auto records = executor.get_records();
    if (records.empty()) {
        std::fprintf(stderr, "No records collected\n");
        return EXIT_FAILURE;
    }

    print_record(records[0]);
    std::printf("POP from pool: %s\n", pool.last_pop().c_str());

    return EXIT_SUCCESS;
}

static int run_benchmark(const BenchmarkConfig& config) {
    BenchmarkRunner runner(config);

    if (!runner.setup()) {
        std::fprintf(stderr, "Benchmark setup failed\n");
        return EXIT_FAILURE;
    }

    auto records = runner.run();

    std::printf("\n=== Benchmark Results ===\n");
    std::printf("Total samples:    %zu\n", records.size());

    // Separate warm and reconnect samples
    size_t warm_count = 0;
    size_t reconnect_count = 0;
    for (const auto& rec : records) {
        if (rec.is_reconnect) ++reconnect_count;
        else ++warm_count;
    }
    std::printf("Warm samples:     %zu\n", warm_count);
    std::printf("Reconnect samples:%zu\n", reconnect_count);
    std::printf("POP:              %s\n", runner.last_pop().c_str());

    // Compute stats (reconnects are filtered by StatsAggregator)
    StatsAggregator stats;
    for (const auto& rec : records) {
        stats.add(rec);
    }
    auto report = stats.compute();

    if (report.sample_count == 0) {
        std::printf("No warm samples to report.\n");
        return EXIT_SUCCESS;
    }

    std::printf("\n--- Percentiles (warm samples only) ---\n");
    print_percentile_set("queue_delay:", report.queue_delay);
    print_percentile_set("prep_time:", report.prep_time);
    print_percentile_set("trigger_to_wire:", report.trigger_to_wire);
    print_percentile_set("write_duration:", report.write_duration);
    print_percentile_set("write_to_first_byte:", report.write_to_first_byte);
    print_percentile_set("warm_ttfb:", report.warm_ttfb);
    print_percentile_set("trigger_to_first_byte:", report.trigger_to_first_byte);

    // Print individual records if few samples
    if (records.size() <= 10) {
        std::printf("\n--- Individual Records ---\n");
        for (size_t i = 0; i < records.size(); ++i) {
            std::printf("\nSample %zu:\n", i + 1);
            print_record(records[i]);
        }
    }

    return EXIT_SUCCESS;
}

int main(int argc, char* argv[]) {
    bool trigger_test = false;
    bool benchmark = false;
    BenchmarkConfig config;

    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--trigger-test") == 0) {
            trigger_test = true;
        } else if (std::strcmp(argv[i], "--benchmark") == 0) {
            benchmark = true;
        } else if (std::strcmp(argv[i], "--mode") == 0 && i + 1 < argc) {
            ++i;
            if (std::strcmp(argv[i], "single-shot") == 0) {
                config.mode = BenchmarkMode::SingleShot;
            } else if (std::strcmp(argv[i], "random-cadence") == 0) {
                config.mode = BenchmarkMode::RandomCadence;
            } else if (std::strcmp(argv[i], "burst-race") == 0) {
                config.mode = BenchmarkMode::BurstRace;
            } else {
                std::fprintf(stderr, "Unknown mode: %s\n", argv[i]);
                return EXIT_FAILURE;
            }
        } else if (std::strcmp(argv[i], "--samples") == 0 && i + 1 < argc) {
            config.sample_count = static_cast<size_t>(std::atoi(argv[++i]));
        } else if (std::strcmp(argv[i], "--connections") == 0 && i + 1 < argc) {
            config.pool_size = static_cast<size_t>(std::atoi(argv[++i]));
        } else if (std::strcmp(argv[i], "--burst-size") == 0 && i + 1 < argc) {
            config.burst_size = static_cast<size_t>(std::atoi(argv[++i]));
        } else if (std::strcmp(argv[i], "--min-interval") == 0 && i + 1 < argc) {
            config.min_interval_ms = static_cast<uint32_t>(std::atoi(argv[++i]));
        } else if (std::strcmp(argv[i], "--max-interval") == 0 && i + 1 < argc) {
            config.max_interval_ms = static_cast<uint32_t>(std::atoi(argv[++i]));
        } else if (std::strcmp(argv[i], "--pin-core") == 0 && i + 1 < argc) {
            config.pin_core = std::atoi(argv[++i]);
        } else if (std::strcmp(argv[i], "--help") == 0 || std::strcmp(argv[i], "-h") == 0) {
            print_usage();
            return EXIT_SUCCESS;
        }
    }

    if (trigger_test) {
        return run_trigger_test();
    }

    if (benchmark) {
        return run_benchmark(config);
    }

    print_usage();
    return EXIT_SUCCESS;
}
