#pragma once

#include "connection/connection_pool.h"
#include <thread>
#include <atomic>
#include <chrono>
#include <functional>

namespace rtt {

// Maintenance thread: periodic health checks, reconnects, POP verification.
class MaintenanceThread {
public:
    using PopChangeCallback = std::function<void(const std::string& old_pop, const std::string& new_pop)>;

    explicit MaintenanceThread(ConnectionPool& pool,
                                std::chrono::milliseconds health_check_interval = std::chrono::seconds(5));
    ~MaintenanceThread();

    // Non-copyable
    MaintenanceThread(const MaintenanceThread&) = delete;
    MaintenanceThread& operator=(const MaintenanceThread&) = delete;

    void start();
    void stop();
    bool is_running() const { return running_.load(std::memory_order_relaxed); }

    // Set callback for POP changes
    void on_pop_change(PopChangeCallback cb) { pop_change_cb_ = std::move(cb); }

    // Get counts for observability
    size_t health_check_count() const { return health_check_count_.load(); }
    size_t reconnect_count() const { return reconnect_count_.load(); }

    // Run one cycle (for testing)
    void run_once();

private:
    ConnectionPool& pool_;
    std::chrono::milliseconds interval_;

    std::atomic<bool> running_{false};
    std::atomic<bool> stop_flag_{false};
    std::thread thread_;

    std::string last_pop_;
    PopChangeCallback pop_change_cb_;

    std::atomic<size_t> health_check_count_{0};
    std::atomic<size_t> reconnect_count_{0};

    void run();
};

} // namespace rtt
