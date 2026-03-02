#pragma once

#include <cstdint>
#include <cstring>

namespace rtt {

// Per-request timestamp record with 8 checkpoints from AGENTS.md section E.
// All timestamps are in nanoseconds from a monotonic clock.
struct TimestampRecord {
    uint64_t t_trigger_rx      = 0;  // Trigger received by executor
    uint64_t t_dispatch_q      = 0;  // Trigger placed on execution queue
    uint64_t t_exec_start      = 0;  // Execution thread begins processing
    uint64_t t_buf_ready       = 0;  // Request buffer fully patched and ready
    uint64_t t_write_begin     = 0;  // First call into send/write path
    uint64_t t_write_end       = 0;  // Request write completed
    uint64_t t_first_resp_byte = 0;  // First response byte received
    uint64_t t_headers_done    = 0;  // Full response headers parsed

    bool is_reconnect = false;       // True if a reconnect happened for this request
    char cf_ray_pop[4] = {};         // Cloudflare POP code (e.g., "EWR")

    // Derived metrics (all in nanoseconds)
    uint64_t queue_delay()          const { return t_exec_start - t_trigger_rx; }
    uint64_t prep_time()            const { return t_buf_ready - t_exec_start; }
    uint64_t trigger_to_wire()      const { return t_write_begin - t_trigger_rx; }
    uint64_t write_duration()       const { return t_write_end - t_write_begin; }
    uint64_t write_to_first_byte()  const { return t_first_resp_byte - t_write_end; }
    uint64_t warm_ttfb()            const { return t_first_resp_byte - t_write_begin; }
    uint64_t trigger_to_first_byte() const { return t_first_resp_byte - t_trigger_rx; }
};

} // namespace rtt
