#include "executor/ingress_thread.h"

namespace rtt {

IngressThread::IngressThread(TriggerQueue& queue) : queue_(queue) {}

IngressThread::~IngressThread() {
    stop();
}

bool IngressThread::inject(TriggerMessage msg) {
    msg.t_trigger_rx = MonotonicClock::now();
    return queue_.push(msg);
}

void IngressThread::start() {
    if (running_.load()) return;
    stop_flag_.store(false);
    thread_ = std::thread(&IngressThread::run, this);
    running_.store(true);
}

void IngressThread::stop() {
    stop_flag_.store(true, std::memory_order_release);
    if (thread_.joinable()) {
        thread_.join();
    }
    running_.store(false);
}

void IngressThread::run() {
    // For now, the ingress thread is a simple idle loop.
    // In production, this would listen on a UDS or shared memory.
    // Triggers are injected via inject() from external callers.
    while (!stop_flag_.load(std::memory_order_acquire)) {
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
}

} // namespace rtt
