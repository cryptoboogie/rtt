#include <gtest/gtest.h>
#include "connection/tcp_connector.h"
#include <unistd.h>
#include <arpa/inet.h>

using namespace rtt;

TEST(TcpConnector, ResolveHost) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443);
    ASSERT_FALSE(addrs.empty()) << "DNS resolution returned no results";

    bool has_v4 = false, has_v6 = false;
    for (const auto& a : addrs) {
        if (a.family == AF_INET) has_v4 = true;
        if (a.family == AF_INET6) has_v6 = true;
    }
    // At minimum one family should be present
    EXPECT_TRUE(has_v4 || has_v6);
}

TEST(TcpConnector, ForceAddressFamilyIPv4) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443, AddressFamily::V4);
    for (const auto& a : addrs) {
        EXPECT_EQ(a.family, AF_INET);
    }
}

TEST(TcpConnector, ForceAddressFamilyIPv6) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443, AddressFamily::V6);
    for (const auto& a : addrs) {
        EXPECT_EQ(a.family, AF_INET6);
    }
}

TEST(TcpConnector, ConnectToHost) {
    int fd = TcpConnector::connect_to_host("clob.polymarket.com", 443);
    ASSERT_GE(fd, 0) << "Failed to connect to clob.polymarket.com:443";
    ::close(fd);
}

TEST(TcpConnector, ConnectFailsGracefully) {
    // Non-routable address should timeout, not crash
    ResolvedAddress bad{};
    auto* sa = reinterpret_cast<struct sockaddr_in*>(&bad.addr);
    sa->sin_family = AF_INET;
    sa->sin_port = htons(9999);
    // 192.0.2.1 is TEST-NET-1, should be non-routable
    inet_pton(AF_INET, "192.0.2.1", &sa->sin_addr);
    bad.addr_len = sizeof(struct sockaddr_in);
    bad.family = AF_INET;

    int fd = TcpConnector::connect(bad, 1000); // 1s timeout
    EXPECT_EQ(fd, -1);
}

TEST(TcpConnector, ResolvedAddressToString) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443, AddressFamily::V4);
    if (!addrs.empty()) {
        std::string s = addrs[0].to_string();
        EXPECT_FALSE(s.empty());
        EXPECT_NE(s.find(":443"), std::string::npos);
    }
}
