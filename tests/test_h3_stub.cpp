#include <gtest/gtest.h>
#include "connection/h3_stub.h"

using namespace rtt;

TEST(H3Stub, StatusIsNotImplemented) {
    EXPECT_EQ(h3_status(), H3Status::NotImplemented);
}

TEST(H3Stub, ProbeDetectsH3Advertisement) {
    auto result = probe_h3_support("clob.polymarket.com", 443);

    // The endpoint is known to advertise HTTP/3 via alt-svc
    EXPECT_TRUE(result.alt_svc_advertised)
        << "Expected alt-svc h3 advertisement";
    EXPECT_FALSE(result.alt_svc_value.empty());

    std::printf("  alt-svc: %s\n", result.alt_svc_value.c_str());
    std::printf("  h3 advertised: %s\n",
                result.alt_svc_advertised ? "yes" : "no");
}
