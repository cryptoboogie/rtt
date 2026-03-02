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

int main(int argc, char* argv[]) {
    bool trigger_test = false;

    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--trigger-test") == 0) {
            trigger_test = true;
        }
    }

    if (!trigger_test) {
        std::printf("Usage: rtt-executor --trigger-test\n");
        std::printf("       rtt-executor --benchmark [options]\n");
        return EXIT_SUCCESS;
    }

    // --- Trigger test mode ---
    std::printf("Warming up connections...\n");
    ConnectionPool pool("clob.polymarket.com", 443, 2);
    size_t warmed = pool.warmup();
    std::printf("Warmed %zu connections\n", warmed);
    if (warmed == 0) {
        std::fprintf(stderr, "Failed to establish any connections\n");
        return EXIT_FAILURE;
    }

    // Prepare request template
    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "clob.polymarket.com", "");

    // Create queue and threads
    TriggerQueue queue;
    IngressThread ingress(queue);
    ExecutionThread executor(queue, pool, tmpl);

    // Start execution thread
    executor.start();

    // Inject a single trigger
    std::printf("Injecting trigger...\n");
    ingress.inject(TriggerMessage::create(1));

    // Wait for result
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
