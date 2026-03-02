#include <gtest/gtest.h>
#include "benchmark/benchmark_runner.h"
#include "connection/tcp_connector.h"
#include <cstring>

using namespace rtt;

TEST(AddressFamily, IPv4ResolvesAndConnects) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443, AddressFamily::V4);
    ASSERT_FALSE(addrs.empty()) << "No IPv4 addresses resolved";
    for (const auto& a : addrs) {
        EXPECT_EQ(a.family, AF_INET);
    }

    int fd = TcpConnector::connect(addrs[0], 5000);
    ASSERT_GE(fd, 0) << "IPv4 connect failed";
    close(fd);
}

TEST(AddressFamily, IPv6ResolvesAndConnects) {
    auto addrs = TcpConnector::resolve("clob.polymarket.com", 443, AddressFamily::V6);
    ASSERT_FALSE(addrs.empty()) << "No IPv6 addresses resolved";
    for (const auto& a : addrs) {
        EXPECT_EQ(a.family, AF_INET6);
    }

    int fd = TcpConnector::connect(addrs[0], 5000);
    ASSERT_GE(fd, 0) << "IPv6 connect failed";
    close(fd);
}

TEST(AddressFamily, BenchmarkWithIPv4) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 2;
    config.pool_size = 1;
    config.address_family = AddressFamily::V4;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    for (const auto& rec : records) {
        if (rec.is_reconnect) continue;
        EXPECT_GT(rec.t_first_resp_byte, 0u);
        EXPECT_GT(std::strlen(rec.cf_ray_pop), 0u);
    }
}

TEST(AddressFamily, BenchmarkWithIPv6) {
    BenchmarkConfig config;
    config.mode = BenchmarkMode::SingleShot;
    config.sample_count = 2;
    config.pool_size = 1;
    config.address_family = AddressFamily::V6;

    BenchmarkRunner runner(config);
    ASSERT_TRUE(runner.setup());

    auto records = runner.run();
    ASSERT_GE(records.size(), 1u);

    for (const auto& rec : records) {
        if (rec.is_reconnect) continue;
        EXPECT_GT(rec.t_first_resp_byte, 0u);
        EXPECT_GT(std::strlen(rec.cf_ray_pop), 0u);
    }
}
