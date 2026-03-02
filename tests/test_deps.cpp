#include <gtest/gtest.h>
#include <openssl/opensslv.h>
#include <openssl/ssl.h>
#include <nghttp2/nghttp2.h>

TEST(Deps, OpenSSLVersion) {
    EXPECT_NE(OpenSSL_version(OPENSSL_VERSION), nullptr);
    // Verify we have OpenSSL (not LibreSSL) with ALPN support
    EXPECT_GE(OPENSSL_VERSION_NUMBER, 0x10100000L);
}

TEST(Deps, Nghttp2Version) {
    auto* info = nghttp2_version(0);
    EXPECT_NE(info, nullptr);
    EXPECT_GT(info->version_num, 0);
}
