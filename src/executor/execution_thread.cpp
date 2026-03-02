#include "executor/execution_thread.h"
#include <cstring>

namespace rtt {

ExecutionThread::ExecutionThread(TriggerQueue& queue, ConnectionPool& pool,
                                 RequestTemplate tmpl)
    : queue_(queue), pool_(pool), template_(std::move(tmpl)) {
    records_.reserve(10000);
}

ExecutionThread::~ExecutionThread() {
    stop();
}

TimestampRecord ExecutionThread::process_one(const TriggerMessage& trigger) {
    TimestampRecord rec{};
    rec.t_trigger_rx = trigger.t_trigger_rx;
    rec.t_dispatch_q = MonotonicClock::now();
    rec.t_exec_start = MonotonicClock::now();

    // Patch template with trigger payload (if needed)
    // For now, template is used as-is since auth mechanism is unspecified
    rec.t_buf_ready = MonotonicClock::now();

    // Acquire warm connection
    H2Session* session = pool_.acquire();
    if (!session) {
        rec.is_reconnect = true;
        return rec;
    }

    // Submit request
    auto headers = template_.headers();
    const void* body = template_.body_size() > 0 ? template_.body_data() : nullptr;

    rec.t_write_begin = MonotonicClock::now();

    int32_t stream_id = session->submit_request(headers, body, template_.body_size());
    if (stream_id < 0) {
        rec.is_reconnect = true;
        return rec;
    }

    rec.t_write_end = MonotonicClock::now();

    // Wait for response
    auto response = session->run_until_response(stream_id);
    rec.t_first_resp_byte = MonotonicClock::now();
    rec.t_headers_done = MonotonicClock::now();

    // Extract cf-ray POP
    std::string cf_ray = response.get_header("cf-ray");
    if (!cf_ray.empty()) {
        auto pos = cf_ray.rfind('-');
        if (pos != std::string::npos) {
            std::string pop = cf_ray.substr(pos + 1);
            size_t copy_len = std::min(pop.size(), sizeof(rec.cf_ray_pop) - 1);
            std::memcpy(rec.cf_ray_pop, pop.c_str(), copy_len);
        }
    }

    return rec;
}

void ExecutionThread::start() {
    if (running_.load()) return;
    stop_flag_.store(false);
    thread_ = std::thread(&ExecutionThread::run, this);
    running_.store(true);
}

void ExecutionThread::stop() {
    stop_flag_.store(true, std::memory_order_release);
    if (thread_.joinable()) {
        thread_.join();
    }
    running_.store(false);
}

void ExecutionThread::run() {
    while (!stop_flag_.load(std::memory_order_acquire)) {
        auto msg = queue_.pop();
        if (!msg.has_value()) {
            // Brief spin-wait, not sleep — production would use futex/eventfd
            std::this_thread::yield();
            continue;
        }

        auto rec = process_one(*msg);

        std::lock_guard<std::mutex> lock(records_mutex_);
        records_.push_back(rec);
    }
}

std::vector<TimestampRecord> ExecutionThread::get_records() const {
    std::lock_guard<std::mutex> lock(records_mutex_);
    return records_;
}

void ExecutionThread::clear_records() {
    std::lock_guard<std::mutex> lock(records_mutex_);
    records_.clear();
}

size_t ExecutionThread::processed_count() const {
    std::lock_guard<std::mutex> lock(records_mutex_);
    return records_.size();
}

} // namespace rtt
