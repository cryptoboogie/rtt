#pragma once

#include "connection/h2_session.h"
#include "connection/tcp_connector.h"
#include <vector>
#include <string>
#include <mutex>
#include <atomic>

namespace rtt {

// Connection pool maintaining warm HTTP/2 connections.
// Thread-safe for concurrent acquire/health_check/reconnect from different threads.
class ConnectionPool {
public:
    ConnectionPool(const std::string& hostname, uint16_t port, size_t pool_size = 2,
                   AddressFamily af = AddressFamily::AUTO);
    ~ConnectionPool() = default;

    // Non-copyable
    ConnectionPool(const ConnectionPool&) = delete;
    ConnectionPool& operator=(const ConnectionPool&) = delete;

    // Establish all connections. Returns number of successful connections.
    size_t warmup();

    // Acquire a warm session for use. Returns pointer to H2Session or nullptr.
    // Round-robin selection among healthy connections.
    H2Session* acquire();

    // Run health check on all connections. Returns number of healthy connections.
    size_t health_check();

    // Reconnect a specific connection by index. Returns true on success.
    bool reconnect(size_t index);

    // Get number of healthy connections
    size_t healthy_count() const;

    // Get pool size
    size_t pool_size() const { return pool_size_; }

    // Get the last cf-ray POP observed from health checks
    std::string last_pop() const;

private:
    struct Connection {
        H2Session session;
        bool healthy = false;
    };

    std::string hostname_;
    uint16_t port_;
    size_t pool_size_;
    AddressFamily address_family_;

    std::vector<Connection> connections_;
    std::atomic<size_t> next_index_{0};
    mutable std::mutex mutex_;

    bool establish_connection(size_t index);
};

} // namespace rtt
