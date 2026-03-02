#pragma once

#include "queue/spsc_queue.h"
#include "trigger/trigger_message.h"
#include "clock/monotonic_clock.h"
#include <thread>
#include <atomic>
#include <functional>

namespace rtt {

// Queue type for trigger delivery
using TriggerQueue = SPSCQueue<TriggerMessage, 1024>;

// Ingress thread: receives triggers and enqueues them with timestamps.
class IngressThread {
public:
    explicit IngressThread(TriggerQueue& queue);
    ~IngressThread();

    // Non-copyable
    IngressThread(const IngressThread&) = delete;
    IngressThread& operator=(const IngressThread&) = delete;

    // Inject a trigger (can be called from any thread).
    // Sets t_trigger_rx timestamp and pushes to queue.
    bool inject(TriggerMessage msg);

    // Start the ingress thread (for future external trigger sources like UDS).
    void start();

    // Stop the ingress thread.
    void stop();

    bool is_running() const { return running_.load(std::memory_order_relaxed); }

private:
    TriggerQueue& queue_;
    std::atomic<bool> running_{false};
    std::atomic<bool> stop_flag_{false};
    std::thread thread_;

    void run();
};

} // namespace rtt
