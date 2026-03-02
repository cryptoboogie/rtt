#include <gtest/gtest.h>
#include "request/request_template.h"
#include <string>
#include <string_view>

using namespace rtt;

static std::string_view nv_name(const nghttp2_nv& nv) {
    return {reinterpret_cast<const char*>(nv.name), nv.namelen};
}

static std::string_view nv_value(const nghttp2_nv& nv) {
    return {reinterpret_cast<const char*>(nv.value), nv.valuelen};
}

TEST(RequestTemplate, PrecomputedHeadersPresent) {
    RequestTemplate tmpl;
    tmpl.prepare("POST", "/order", "clob.polymarket.com");

    auto hdrs = tmpl.headers();
    ASSERT_GE(hdrs.size(), 5u);

    EXPECT_EQ(nv_name(hdrs[0]), ":method");
    EXPECT_EQ(nv_value(hdrs[0]), "POST");

    EXPECT_EQ(nv_name(hdrs[1]), ":path");
    EXPECT_EQ(nv_value(hdrs[1]), "/order");

    EXPECT_EQ(nv_name(hdrs[2]), ":scheme");
    EXPECT_EQ(nv_value(hdrs[2]), "https");

    EXPECT_EQ(nv_name(hdrs[3]), ":authority");
    EXPECT_EQ(nv_value(hdrs[3]), "clob.polymarket.com");

    EXPECT_EQ(nv_name(hdrs[4]), "content-type");
    EXPECT_EQ(nv_value(hdrs[4]), "application/json");
}

TEST(RequestTemplate, AddCustomHeader) {
    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "example.com", "");
    tmpl.add_header("x-custom", "value123");

    auto hdrs = tmpl.headers();
    bool found = false;
    for (auto& h : hdrs) {
        if (nv_name(h) == "x-custom") {
            EXPECT_EQ(nv_value(h), "value123");
            found = true;
        }
    }
    EXPECT_TRUE(found) << "Custom header not found";
}

TEST(RequestTemplate, BodySetAndRead) {
    RequestTemplate tmpl;
    tmpl.prepare("POST", "/", "example.com");
    tmpl.set_body(R"({"market":"abc","price":0.5})");

    std::string_view body(tmpl.body_data(), tmpl.body_size());
    EXPECT_EQ(body, R"({"market":"abc","price":0.5})");
}

TEST(RequestTemplate, DynamicBodyPatch) {
    RequestTemplate tmpl;
    tmpl.prepare("POST", "/", "example.com");
    // Body with a placeholder region for price
    std::string body_template = R"({"market":"abc","price":"XXXX"})";
    tmpl.set_body(body_template);

    // Find the offset of XXXX
    size_t offset = body_template.find("XXXX");
    ASSERT_NE(offset, std::string::npos);

    int slot = tmpl.register_body_patch(offset, 4);
    ASSERT_GE(slot, 0);

    // Patch with a new value
    tmpl.patch(slot, "0.75");

    std::string_view body(tmpl.body_data(), tmpl.body_size());
    EXPECT_NE(body.find("0.75"), std::string_view::npos);
    EXPECT_EQ(body.find("XXXX"), std::string_view::npos);
}

TEST(RequestTemplate, DynamicHeaderPatch) {
    RequestTemplate tmpl;
    tmpl.prepare("POST", "/", "example.com");
    // Add auth header with placeholder
    tmpl.add_header("authorization", "Bearer PLACEHOLDER_TOKEN_HERE");

    // The auth header is at index 5 (after :method, :path, :scheme, :authority, content-type)
    size_t auth_idx = tmpl.header_count() - 1;

    // Register patch slot starting after "Bearer "
    int slot = tmpl.register_header_patch(auth_idx, 7, 64);
    ASSERT_GE(slot, 0);

    tmpl.patch(slot, "real_token_abc123");

    auto hdrs = tmpl.headers();
    std::string_view auth_value = nv_value(hdrs[auth_idx]);
    EXPECT_EQ(auth_value.substr(0, 7), "Bearer ");
    EXPECT_NE(auth_value.find("real_token_abc123"), std::string_view::npos);
}

TEST(RequestTemplate, Nghttp2NvFlagsSet) {
    RequestTemplate tmpl;
    tmpl.prepare("GET", "/", "example.com", "");

    auto hdrs = tmpl.headers();
    for (auto& h : hdrs) {
        // NO_COPY flags should be set since we own the memory
        EXPECT_TRUE(h.flags & NGHTTP2_NV_FLAG_NO_COPY_NAME);
        EXPECT_TRUE(h.flags & NGHTTP2_NV_FLAG_NO_COPY_VALUE);
    }
}
