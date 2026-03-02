#include <gtest/gtest.h>
#include "connection/tcp_connector.h"
#include "connection/tls_session.h"
#include <unistd.h>

using namespace rtt;

class TlsSessionTest : public ::testing::Test {
protected:
    int fd_ = -1;

    void SetUp() override {
        fd_ = TcpConnector::connect_to_host("clob.polymarket.com", 443);
        ASSERT_GE(fd_, 0) << "TCP connect failed — network required";
    }

    void TearDown() override {
        if (fd_ >= 0) ::close(fd_);
    }
};

TEST_F(TlsSessionTest, HandshakeSucceeds) {
    TlsSession tls;
    ASSERT_TRUE(tls.handshake(fd_, "clob.polymarket.com"));
    EXPECT_TRUE(tls.is_valid());
    // Take ownership of fd away from teardown since TLS now owns it
    fd_ = -1;
}

TEST_F(TlsSessionTest, AlpnH2Negotiated) {
    TlsSession tls;
    ASSERT_TRUE(tls.handshake(fd_, "clob.polymarket.com"));
    EXPECT_EQ(tls.negotiated_protocol(), "h2");
    fd_ = -1;
}

TEST_F(TlsSessionTest, TlsVersionIs13) {
    TlsSession tls;
    ASSERT_TRUE(tls.handshake(fd_, "clob.polymarket.com"));
    std::string version = tls.tls_version();
    // Accept TLS 1.3 or 1.2 (server may negotiate either)
    EXPECT_TRUE(version == "TLSv1.3" || version == "TLSv1.2")
        << "Unexpected TLS version: " << version;
    fd_ = -1;
}
