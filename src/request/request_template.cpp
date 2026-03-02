#include "request/request_template.h"
#include <algorithm>

namespace rtt {

size_t RequestTemplate::add_header_internal(std::string_view name, std::string_view value) {
    if (header_count_ >= MAX_HEADERS) return header_count_;

    auto& entry = header_entries_[header_count_];
    size_t nlen = std::min(name.size(), sizeof(entry.name) - 1);
    size_t vlen = std::min(value.size(), sizeof(entry.value) - 1);
    std::memcpy(entry.name, name.data(), nlen);
    entry.name_len = nlen;
    std::memcpy(entry.value, value.data(), vlen);
    entry.value_len = vlen;

    return header_count_++;
}

void RequestTemplate::rebuild_nv_array() {
    for (size_t i = 0; i < header_count_; ++i) {
        auto& entry = header_entries_[i];
        auto& nv = nv_headers_[i];
        nv.name = reinterpret_cast<uint8_t*>(entry.name);
        nv.namelen = entry.name_len;
        nv.value = reinterpret_cast<uint8_t*>(entry.value);
        nv.valuelen = entry.value_len;
        nv.flags = NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE;
    }
}

void RequestTemplate::prepare(std::string_view method,
                               std::string_view path,
                               std::string_view authority,
                               std::string_view content_type) {
    header_count_ = 0;
    add_header_internal(":method", method);
    add_header_internal(":path", path);
    add_header_internal(":scheme", "https");
    add_header_internal(":authority", authority);
    if (!content_type.empty()) {
        add_header_internal("content-type", content_type);
    }
    rebuild_nv_array();
}

void RequestTemplate::add_header(std::string_view name, std::string_view value) {
    add_header_internal(name, value);
    rebuild_nv_array();
}

void RequestTemplate::set_body(std::string_view body) {
    size_t len = std::min(body.size(), body_.size());
    std::memcpy(body_.data(), body.data(), len);
    body_len_ = len;
}

int RequestTemplate::register_body_patch(size_t offset, size_t length) {
    if (patch_count_ >= MAX_PATCH_SLOTS) return -1;
    if (offset + length > body_.size()) return -1;

    auto& slot = patch_slots_[patch_count_];
    slot.offset = offset;
    slot.length = length;
    slot.in_body = true;
    return static_cast<int>(patch_count_++);
}

int RequestTemplate::register_header_patch(size_t header_index, size_t offset, size_t length) {
    if (patch_count_ >= MAX_PATCH_SLOTS) return -1;
    if (header_index >= header_count_) return -1;
    if (offset + length > sizeof(HeaderEntry::value)) return -1;

    auto& slot = patch_slots_[patch_count_];
    slot.offset = offset;
    slot.length = length;
    slot.in_body = false;
    slot.header_index = header_index;
    return static_cast<int>(patch_count_++);
}

void RequestTemplate::patch(size_t slot_index, std::string_view value) {
    if (slot_index >= patch_count_) return;
    const auto& slot = patch_slots_[slot_index];
    size_t copy_len = std::min(value.size(), slot.length);

    if (slot.in_body) {
        std::memcpy(body_.data() + slot.offset, value.data(), copy_len);
    } else {
        auto& entry = header_entries_[slot.header_index];
        std::memcpy(entry.value + slot.offset, value.data(), copy_len);
        // If the patch changes the effective value length, update it
        // (assumes patch is at the end of the value for auth tokens, etc.)
        if (slot.offset + copy_len > entry.value_len) {
            entry.value_len = slot.offset + copy_len;
        }
        // Update the nv header to reflect new length
        nv_headers_[slot.header_index].valuelen = entry.value_len;
    }
}

std::span<nghttp2_nv> RequestTemplate::headers() {
    return std::span<nghttp2_nv>(nv_headers_.data(), header_count_);
}

} // namespace rtt
