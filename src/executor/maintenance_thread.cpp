#include "executor/maintenance_thread.h"

namespace rtt {

MaintenanceThread::MaintenanceThread(ConnectionPool& pool,
                                       std::chrono::milliseconds health_check_interval)
    : pool_(pool), interval_(health_check_interval) {}

MaintenanceThread::~MaintenanceThread() {
    stop();
}

void MaintenanceThread::run_once() {
    size_t healthy = pool_.health_check();
    health_check_count_.fetch_add(1, std::memory_order_relaxed);

    // Reconnect any unhealthy connections
    for (size_t i = 0; i < pool_.pool_size(); ++i) {
        // Try to reconnect if pool is below full health
        if (healthy < pool_.pool_size()) {
            if (pool_.reconnect(i)) {
                reconnect_count_.fetch_add(1, std::memory_order_relaxed);
                healthy = pool_.healthy_count();
            }
        }
    }

    // Check POP consistency
    std::string current_pop = pool_.last_pop();
    if (!current_pop.empty() && current_pop != last_pop_) {
        if (!last_pop_.empty() && pop_change_cb_) {
            pop_change_cb_(last_pop_, current_pop);
        }
        last_pop_ = current_pop;
    }
}

void MaintenanceThread::start() {
    if (running_.load()) return;
    stop_flag_.store(false);
    thread_ = std::thread(&MaintenanceThread::run, this);
    running_.store(true);
}

void MaintenanceThread::stop() {
    stop_flag_.store(true, std::memory_order_release);
    if (thread_.joinable()) {
        thread_.join();
    }
    running_.store(false);
}

void MaintenanceThread::run() {
    while (!stop_flag_.load(std::memory_order_acquire)) {
        run_once();

        // Sleep in small increments to allow quick shutdown
        auto deadline = std::chrono::steady_clock::now() + interval_;
        while (std::chrono::steady_clock::now() < deadline) {
            if (stop_flag_.load(std::memory_order_acquire)) return;
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
        }
    }
}

} // namespace rtt
