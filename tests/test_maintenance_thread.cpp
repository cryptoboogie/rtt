#include <gtest/gtest.h>
#include "executor/maintenance_thread.h"

using namespace rtt;

class MaintenanceThreadTest : public ::testing::Test {
protected:
    ConnectionPool pool_{"clob.polymarket.com", 443, 2};

    void SetUp() override {
        ASSERT_GE(pool_.warmup(), 2u) << "Need 2 warm connections";
    }
};

TEST_F(MaintenanceThreadTest, RunOnceDoesHealthCheck) {
    MaintenanceThread maint(pool_, std::chrono::seconds(5));
    maint.run_once();
    EXPECT_EQ(maint.health_check_count(), 1u);
    EXPECT_EQ(pool_.healthy_count(), 2u);
}

TEST_F(MaintenanceThreadTest, StartsAndStops) {
    MaintenanceThread maint(pool_, std::chrono::milliseconds(100));
    maint.start();
    EXPECT_TRUE(maint.is_running());

    std::this_thread::sleep_for(std::chrono::milliseconds(250));
    maint.stop();
    EXPECT_FALSE(maint.is_running());

    // Should have done at least 1 health check in 250ms
    EXPECT_GE(maint.health_check_count(), 1u);
}

TEST_F(MaintenanceThreadTest, MultipleHealthChecks) {
    MaintenanceThread maint(pool_, std::chrono::milliseconds(100));
    maint.start();

    std::this_thread::sleep_for(std::chrono::milliseconds(350));
    maint.stop();

    EXPECT_GE(maint.health_check_count(), 2u);
}
