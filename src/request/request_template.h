#pragma once

#include <nghttp2/nghttp2.h>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string_view>
#include <span>

namespace rtt {

// Maximum number of HTTP/2 headers in a prebuilt template
static constexpr size_t MAX_HEADERS = 16;
// Maximum body template size
static constexpr size_t MAX_BODY_SIZE = 4096;
// Maximum number of dynamic patch slots
static constexpr size_t MAX_PATCH_SLOTS = 4;
// Maximum header value length
static constexpr size_t MAX_HEADER_VALUE_LEN = 512;

struct PatchSlot {
    size_t offset = 0;   // Offset into the target buffer
    size_t length = 0;   // Maximum length of the patchable region
    bool in_body  = true; // true = body buffer, false = header value buffer
    size_t header_index = 0; // If in header, which header's value
};

// Zero-allocation request template for HTTP/2.
// Precomputes all headers and body. At trigger time, only patch dynamic fields.
class RequestTemplate {
public:
    RequestTemplate() = default;

    // Prepare the template with static values. Call once at init time.
    void prepare(std::string_view method,
                 std::string_view path,
                 std::string_view authority,
                 std::string_view content_type = "application/json");

    // Add a custom static header
    void add_header(std::string_view name, std::string_view value);

    // Set the body template content
    void set_body(std::string_view body);

    // Register a dynamic patch slot in the body
    // Returns slot index, or -1 if no slots available
    int register_body_patch(size_t offset, size_t length);

    // Register a dynamic patch slot in a header value
    int register_header_patch(size_t header_index, size_t offset, size_t length);

    // Patch a dynamic slot with new content (zero-allocation on hot path)
    void patch(size_t slot_index, std::string_view value);

    // Get the nghttp2 header array for submission
    std::span<nghttp2_nv> headers();

    // Get the body data
    const char* body_data() const { return body_.data(); }
    size_t body_size() const { return body_len_; }

    size_t header_count() const { return header_count_; }

private:
    // Header storage: names and values stored in fixed arrays
    struct HeaderEntry {
        char name[64] = {};
        char value[MAX_HEADER_VALUE_LEN] = {};
        size_t name_len = 0;
        size_t value_len = 0;
    };

    std::array<HeaderEntry, MAX_HEADERS> header_entries_{};
    std::array<nghttp2_nv, MAX_HEADERS> nv_headers_{};
    size_t header_count_ = 0;

    std::array<char, MAX_BODY_SIZE> body_{};
    size_t body_len_ = 0;

    std::array<PatchSlot, MAX_PATCH_SLOTS> patch_slots_{};
    size_t patch_count_ = 0;

    void rebuild_nv_array();
    size_t add_header_internal(std::string_view name, std::string_view value);
};

} // namespace rtt
