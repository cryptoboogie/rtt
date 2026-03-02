#include "connection/tcp_connector.h"

#include <netdb.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <cerrno>
#include <cstring>
#include <sstream>

namespace rtt {

std::string ResolvedAddress::to_string() const {
    char buf[INET6_ADDRSTRLEN] = {};
    if (family == AF_INET) {
        auto* sa = reinterpret_cast<const struct sockaddr_in*>(&addr);
        inet_ntop(AF_INET, &sa->sin_addr, buf, sizeof(buf));
        return std::string(buf) + ":" + std::to_string(ntohs(sa->sin_port));
    } else {
        auto* sa = reinterpret_cast<const struct sockaddr_in6*>(&addr);
        inet_ntop(AF_INET6, &sa->sin6_addr, buf, sizeof(buf));
        return std::string("[") + buf + "]:" + std::to_string(ntohs(sa->sin6_port));
    }
}

std::vector<ResolvedAddress> TcpConnector::resolve(
    const std::string& hostname, uint16_t port, AddressFamily af) {

    struct addrinfo hints{};
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;

    switch (af) {
        case AddressFamily::V4: hints.ai_family = AF_INET; break;
        case AddressFamily::V6: hints.ai_family = AF_INET6; break;
        case AddressFamily::AUTO: hints.ai_family = AF_UNSPEC; break;
    }

    std::string port_str = std::to_string(port);
    struct addrinfo* result = nullptr;
    int err = getaddrinfo(hostname.c_str(), port_str.c_str(), &hints, &result);
    if (err != 0 || !result) {
        return {};
    }

    std::vector<ResolvedAddress> addrs;
    for (auto* rp = result; rp != nullptr; rp = rp->ai_next) {
        ResolvedAddress ra{};
        std::memcpy(&ra.addr, rp->ai_addr, rp->ai_addrlen);
        ra.addr_len = static_cast<socklen_t>(rp->ai_addrlen);
        ra.family = rp->ai_family;
        addrs.push_back(ra);
    }

    freeaddrinfo(result);
    return addrs;
}

int TcpConnector::connect(const ResolvedAddress& addr, int timeout_ms) {
    int fd = ::socket(addr.family, SOCK_STREAM, IPPROTO_TCP);
    if (fd < 0) return -1;

    // Set non-blocking for connect with timeout
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
        ::close(fd);
        return -1;
    }

    int ret = ::connect(fd, reinterpret_cast<const struct sockaddr*>(&addr.addr), addr.addr_len);
    if (ret < 0 && errno != EINPROGRESS) {
        ::close(fd);
        return -1;
    }

    if (ret < 0) {
        // Wait for connect to complete
        struct pollfd pfd{};
        pfd.fd = fd;
        pfd.events = POLLOUT;

        ret = ::poll(&pfd, 1, timeout_ms);
        if (ret <= 0) {
            ::close(fd);
            return -1;
        }

        // Check for connect error
        int err = 0;
        socklen_t errlen = sizeof(err);
        if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &errlen) < 0 || err != 0) {
            ::close(fd);
            return -1;
        }
    }

    // Set back to blocking
    fcntl(fd, F_SETFL, flags);

    // Set TCP_NODELAY
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));

    return fd;
}

int TcpConnector::connect_to_host(
    const std::string& hostname, uint16_t port, AddressFamily af, int timeout_ms) {

    auto addrs = resolve(hostname, port, af);
    for (const auto& addr : addrs) {
        int fd = connect(addr, timeout_ms);
        if (fd >= 0) return fd;
    }
    return -1;
}

} // namespace rtt
