#pragma once

#include "queue/spsc_queue.h"
#include "trigger/trigger_message.h"
#include "metrics/timestamp_record.h"
#include "request/request_template.h"
#include "connection/connection_pool.h"
#include "clock/monotonic_clock.h"
#include <thread>
#include <atomic>
#include <vector>
#include <mutex>

namespace rtt {

using TriggerQueue = SPSCQueue<TriggerMessage, 1024>;

// Execution thread: dequeues triggers, patches request template,
// sends on warm connection, records all timestamps.
class ExecutionThread {
public:
    ExecutionThread(TriggerQueue& queue, ConnectionPool& pool,
                    RequestTemplate tmpl);
    ~ExecutionThread();

    // Non-copyable
    ExecutionThread(const ExecutionThread&) = delete;
    ExecutionThread& operator=(const ExecutionThread&) = delete;

    void start();
    void stop();
    bool is_running() const { return running_.load(std::memory_order_relaxed); }

    // Process a single trigger synchronously (for testing without threading).
    // Returns the completed TimestampRecord.
    TimestampRecord process_one(const TriggerMessage& trigger);

    // Get collected timestamp records (thread-safe copy).
    std::vector<TimestampRecord> get_records() const;

    // Clear collected records.
    void clear_records();

    // Get number of processed triggers.
    size_t processed_count() const;

private:
    TriggerQueue& queue_;
    ConnectionPool& pool_;
    RequestTemplate template_;

    std::atomic<bool> running_{false};
    std::atomic<bool> stop_flag_{false};
    std::thread thread_;

    mutable std::mutex records_mutex_;
    std::vector<TimestampRecord> records_;

    void run();
};

} // namespace rtt
