#pragma once

#include <string>
#include <vector>
#include <cstdint>
#include <sys/socket.h>
#include <netinet/in.h>

namespace rtt {

enum class AddressFamily {
    AUTO,  // System default (happy eyeballs or OS preference)
    V4,    // Force IPv4
    V6,    // Force IPv6
};

struct ResolvedAddress {
    struct sockaddr_storage addr;
    socklen_t addr_len;
    int family;  // AF_INET or AF_INET6

    std::string to_string() const;
};

// TCP connector with DNS resolution and non-blocking connect with timeout.
class TcpConnector {
public:
    // Resolve hostname to one or more addresses
    static std::vector<ResolvedAddress> resolve(
        const std::string& hostname,
        uint16_t port,
        AddressFamily af = AddressFamily::AUTO);

    // Connect to a resolved address with timeout. Returns socket fd or -1.
    static int connect(const ResolvedAddress& addr, int timeout_ms = 5000);

    // Convenience: resolve and connect to the first matching address
    static int connect_to_host(
        const std::string& hostname,
        uint16_t port,
        AddressFamily af = AddressFamily::AUTO,
        int timeout_ms = 5000);
};

} // namespace rtt
