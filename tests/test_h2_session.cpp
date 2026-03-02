#include <gtest/gtest.h>
#include "connection/tcp_connector.h"
#include "connection/tls_session.h"
#include "connection/h2_session.h"
#include <unistd.h>

using namespace rtt;

class H2SessionTest : public ::testing::Test {
protected:
    H2Session h2_;

    void SetUp() override {
        int fd = TcpConnector::connect_to_host("clob.polymarket.com", 443);
        ASSERT_GE(fd, 0) << "TCP connect failed";

        TlsSession tls;
        ASSERT_TRUE(tls.handshake(fd, "clob.polymarket.com")) << "TLS handshake failed";
        ASSERT_EQ(tls.negotiated_protocol(), "h2") << "ALPN h2 not negotiated";

        ASSERT_TRUE(h2_.init(std::move(tls))) << "H2 session init failed";
    }
};

TEST_F(H2SessionTest, SessionIsValid) {
    EXPECT_TRUE(h2_.is_valid());
}

TEST_F(H2SessionTest, SimpleGetRequest) {
    // Build a minimal GET request
    nghttp2_nv headers[] = {
        {(uint8_t*)":method", (uint8_t*)"GET", 7, 3, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":path", (uint8_t*)"/", 5, 1, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":scheme", (uint8_t*)"https", 7, 5, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":authority", (uint8_t*)"clob.polymarket.com", 10, 19, NGHTTP2_NV_FLAG_NONE},
    };

    int32_t stream_id = h2_.submit_request(
        std::span<nghttp2_nv>(headers, 4));
    ASSERT_GT(stream_id, 0);

    auto resp = h2_.run_until_response(stream_id);
    EXPECT_TRUE(resp.complete);
    EXPECT_GT(resp.status_code, 0);
}

TEST_F(H2SessionTest, ResponseContainsCfRay) {
    nghttp2_nv headers[] = {
        {(uint8_t*)":method", (uint8_t*)"GET", 7, 3, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":path", (uint8_t*)"/", 5, 1, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":scheme", (uint8_t*)"https", 7, 5, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":authority", (uint8_t*)"clob.polymarket.com", 10, 19, NGHTTP2_NV_FLAG_NONE},
    };

    int32_t stream_id = h2_.submit_request(
        std::span<nghttp2_nv>(headers, 4));
    ASSERT_GT(stream_id, 0);

    auto resp = h2_.run_until_response(stream_id);
    EXPECT_TRUE(resp.complete);

    std::string cf_ray = resp.get_header("cf-ray");
    EXPECT_FALSE(cf_ray.empty()) << "cf-ray header not found in response";
    EXPECT_FALSE(h2_.last_cf_ray().empty());
}

TEST_F(H2SessionTest, SessionIsReusable) {
    // First request
    nghttp2_nv headers[] = {
        {(uint8_t*)":method", (uint8_t*)"GET", 7, 3, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":path", (uint8_t*)"/", 5, 1, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":scheme", (uint8_t*)"https", 7, 5, NGHTTP2_NV_FLAG_NONE},
        {(uint8_t*)":authority", (uint8_t*)"clob.polymarket.com", 10, 19, NGHTTP2_NV_FLAG_NONE},
    };

    int32_t sid1 = h2_.submit_request(std::span<nghttp2_nv>(headers, 4));
    ASSERT_GT(sid1, 0);
    auto resp1 = h2_.run_until_response(sid1);
    EXPECT_TRUE(resp1.complete);

    // Second request on the same session
    int32_t sid2 = h2_.submit_request(std::span<nghttp2_nv>(headers, 4));
    ASSERT_GT(sid2, 0);
    EXPECT_NE(sid1, sid2); // Different stream IDs
    auto resp2 = h2_.run_until_response(sid2);
    EXPECT_TRUE(resp2.complete);
    EXPECT_GT(resp2.status_code, 0);
}
