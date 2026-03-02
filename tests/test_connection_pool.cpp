#include <gtest/gtest.h>
#include "connection/connection_pool.h"

using namespace rtt;

class ConnectionPoolTest : public ::testing::Test {
protected:
    ConnectionPool pool_{"clob.polymarket.com", 443, 2};

    void SetUp() override {
        size_t warmed = pool_.warmup();
        ASSERT_EQ(warmed, 2u) << "Failed to warm up 2 connections";
    }
};

TEST_F(ConnectionPoolTest, InitializesTwoConnections) {
    EXPECT_EQ(pool_.healthy_count(), 2u);
}

TEST_F(ConnectionPoolTest, AcquireReturnsValidSession) {
    H2Session* session = pool_.acquire();
    ASSERT_NE(session, nullptr);
    EXPECT_TRUE(session->is_valid());
}

TEST_F(ConnectionPoolTest, RoundRobinSelection) {
    H2Session* s1 = pool_.acquire();
    H2Session* s2 = pool_.acquire();
    // With 2 connections, round-robin should give different sessions
    EXPECT_NE(s1, s2);

    // Third acquire should wrap around
    H2Session* s3 = pool_.acquire();
    EXPECT_EQ(s1, s3);
}

TEST_F(ConnectionPoolTest, HealthCheckPasses) {
    size_t healthy = pool_.health_check();
    EXPECT_EQ(healthy, 2u);
}

TEST_F(ConnectionPoolTest, ReconnectsAfterLoss) {
    // Force first connection unhealthy by reconnecting
    EXPECT_TRUE(pool_.reconnect(0));
    EXPECT_EQ(pool_.healthy_count(), 2u);
}
