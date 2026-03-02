#include "connection/connection_pool.h"
#include <unistd.h>

namespace rtt {

ConnectionPool::ConnectionPool(const std::string& hostname, uint16_t port,
                               size_t pool_size, AddressFamily af)
    : hostname_(hostname), port_(port), pool_size_(pool_size), address_family_(af) {
    connections_.resize(pool_size_);
}

bool ConnectionPool::establish_connection(size_t index) {
    if (index >= pool_size_) return false;

    int fd = TcpConnector::connect_to_host(hostname_, port_, address_family_);
    if (fd < 0) return false;

    TlsSession tls;
    if (!tls.handshake(fd, hostname_)) {
        ::close(fd);
        return false;
    }

    if (tls.negotiated_protocol() != "h2") {
        return false;
    }

    H2Session h2;
    if (!h2.init(std::move(tls))) {
        return false;
    }

    std::lock_guard<std::mutex> lock(mutex_);
    connections_[index].session = std::move(h2);
    connections_[index].healthy = true;
    return true;
}

size_t ConnectionPool::warmup() {
    size_t success = 0;
    for (size_t i = 0; i < pool_size_; ++i) {
        if (establish_connection(i)) {
            ++success;
        }
    }
    return success;
}

H2Session* ConnectionPool::acquire() {
    size_t start = next_index_.fetch_add(1, std::memory_order_relaxed) % pool_size_;

    std::lock_guard<std::mutex> lock(mutex_);
    // Try round-robin starting from current index
    for (size_t i = 0; i < pool_size_; ++i) {
        size_t idx = (start + i) % pool_size_;
        if (connections_[idx].healthy && connections_[idx].session.is_valid()) {
            return &connections_[idx].session;
        }
    }
    return nullptr;
}

size_t ConnectionPool::health_check() {
    size_t healthy = 0;
    for (size_t i = 0; i < pool_size_; ++i) {
        std::lock_guard<std::mutex> lock(mutex_);
        auto& conn = connections_[i];
        if (!conn.session.is_valid()) {
            conn.healthy = false;
            continue;
        }
        // Send PING to verify connection is alive
        if (conn.session.send_ping()) {
            conn.healthy = true;
            ++healthy;
        } else {
            conn.healthy = false;
        }
    }
    return healthy;
}

bool ConnectionPool::reconnect(size_t index) {
    {
        std::lock_guard<std::mutex> lock(mutex_);
        connections_[index].healthy = false;
        connections_[index].session = H2Session{}; // destroy old session
    }
    return establish_connection(index);
}

size_t ConnectionPool::healthy_count() const {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t count = 0;
    for (const auto& conn : connections_) {
        if (conn.healthy) ++count;
    }
    return count;
}

std::string ConnectionPool::last_pop() const {
    std::lock_guard<std::mutex> lock(mutex_);
    for (const auto& conn : connections_) {
        if (conn.healthy && !conn.session.last_cf_ray().empty()) {
            const auto& ray = conn.session.last_cf_ray();
            // cf-ray format: "id-POP", extract POP after last '-'
            auto pos = ray.rfind('-');
            if (pos != std::string::npos) {
                return ray.substr(pos + 1);
            }
        }
    }
    return "";
}

} // namespace rtt
