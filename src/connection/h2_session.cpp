#include "connection/h2_session.h"
#include <cstring>
#include <algorithm>

namespace rtt {

std::string H2Response::get_header(const std::string& name) const {
    for (const auto& [k, v] : headers) {
        if (k == name) return v;
    }
    return "";
}

H2Session::H2Session() = default;

H2Session::~H2Session() {
    destroy();
}

H2Session::H2Session(H2Session&& other) noexcept
    : session_(other.session_), tls_(std::move(other.tls_)),
      last_cf_ray_(std::move(other.last_cf_ray_)),
      streams_(std::move(other.streams_)) {
    other.session_ = nullptr;
    // Update the user_data pointer in nghttp2 to point to this
    if (session_) {
        nghttp2_session_set_user_data(session_, this);
    }
}

H2Session& H2Session::operator=(H2Session&& other) noexcept {
    if (this != &other) {
        destroy();
        session_ = other.session_;
        tls_ = std::move(other.tls_);
        last_cf_ray_ = std::move(other.last_cf_ray_);
        streams_ = std::move(other.streams_);
        other.session_ = nullptr;
        if (session_) {
            nghttp2_session_set_user_data(session_, this);
        }
    }
    return *this;
}

void H2Session::destroy() {
    if (session_) {
        nghttp2_session_del(session_);
        session_ = nullptr;
    }
    tls_.close();
    streams_.clear();
}

bool H2Session::init(TlsSession&& tls) {
    tls_ = std::move(tls);
    if (!tls_.is_valid()) return false;

    nghttp2_session_callbacks* callbacks = nullptr;
    nghttp2_session_callbacks_new(&callbacks);

    nghttp2_session_callbacks_set_send_callback(callbacks, on_send);
    nghttp2_session_callbacks_set_on_header_callback(callbacks, on_header);
    nghttp2_session_callbacks_set_on_data_chunk_recv_callback(callbacks, on_data_chunk);
    nghttp2_session_callbacks_set_on_stream_close_callback(callbacks, on_stream_close);
    nghttp2_session_callbacks_set_on_frame_recv_callback(callbacks, on_frame_recv);

    int rv = nghttp2_session_client_new(&session_, callbacks, this);
    nghttp2_session_callbacks_del(callbacks);

    if (rv != 0) return false;

    // Send client connection preface (SETTINGS frame)
    nghttp2_settings_entry settings[] = {
        {NGHTTP2_SETTINGS_MAX_CONCURRENT_STREAMS, 100},
        {NGHTTP2_SETTINGS_INITIAL_WINDOW_SIZE, 1048576},
    };
    rv = nghttp2_submit_settings(session_, NGHTTP2_FLAG_NONE,
                                  settings, sizeof(settings) / sizeof(settings[0]));
    if (rv != 0) {
        destroy();
        return false;
    }

    // Flush the SETTINGS frame
    if (!send_pending()) {
        destroy();
        return false;
    }

    // Read server SETTINGS
    if (!recv_data()) {
        destroy();
        return false;
    }

    return true;
}

int32_t H2Session::submit_request(std::span<nghttp2_nv> headers,
                                    const void* body_data, size_t body_len) {
    if (!session_) return -1;

    nghttp2_data_provider* prd_ptr = nullptr;
    nghttp2_data_provider prd{};

    if (body_data && body_len > 0) {
        // We'll store body info in stream state after we know the stream ID,
        // but nghttp2 needs the provider now. Use a temporary approach.
        prd.source.ptr = this;
        prd.read_callback = on_data_source_read;
        prd_ptr = &prd;
    }

    int32_t stream_id = nghttp2_submit_request(
        session_, nullptr, headers.data(), headers.size(), prd_ptr, nullptr);

    if (stream_id < 0) return -1;

    auto& state = streams_[stream_id];
    state.response.stream_id = stream_id;
    state.body_data = body_data;
    state.body_len = body_len;
    state.body_offset = 0;

    return stream_id;
}

H2Response H2Session::run_until_response(int32_t stream_id) {
    auto it = streams_.find(stream_id);
    if (it == streams_.end()) return {};

    // I/O loop: send pending data, receive responses
    while (!it->second.response.complete) {
        if (!send_pending()) break;
        if (!recv_data()) break;
    }

    H2Response response = std::move(it->second.response);
    streams_.erase(it);
    return response;
}

bool H2Session::send_ping() {
    if (!session_) return false;
    uint8_t opaque[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    int rv = nghttp2_submit_ping(session_, NGHTTP2_FLAG_NONE, opaque);
    if (rv != 0) return false;
    return send_pending();
}

bool H2Session::send_pending() {
    int rv = nghttp2_session_send(session_);
    return rv == 0;
}

bool H2Session::recv_data() {
    uint8_t buf[16384];
    int n = tls_.ssl_read(buf, sizeof(buf));
    if (n <= 0) return false;

    ssize_t rv = nghttp2_session_mem_recv(session_, buf, static_cast<size_t>(n));
    return rv >= 0;
}

// --- nghttp2 callbacks ---

ssize_t H2Session::on_send(nghttp2_session* /*session*/, const uint8_t* data,
                            size_t length, int /*flags*/, void* user_data) {
    auto* self = static_cast<H2Session*>(user_data);
    int n = self->tls_.ssl_write(data, length);
    if (n <= 0) return NGHTTP2_ERR_CALLBACK_FAILURE;
    return n;
}

int H2Session::on_header(nghttp2_session* /*session*/, const nghttp2_frame* frame,
                          const uint8_t* name, size_t namelen,
                          const uint8_t* value, size_t valuelen,
                          uint8_t /*flags*/, void* user_data) {
    auto* self = static_cast<H2Session*>(user_data);
    if (frame->hd.type != NGHTTP2_HEADERS) return 0;

    auto it = self->streams_.find(frame->hd.stream_id);
    if (it == self->streams_.end()) return 0;

    std::string hname(reinterpret_cast<const char*>(name), namelen);
    std::string hvalue(reinterpret_cast<const char*>(value), valuelen);

    if (hname == ":status") {
        it->second.response.status_code = std::stoi(hvalue);
    }

    // Track cf-ray
    if (hname == "cf-ray") {
        self->last_cf_ray_ = hvalue;
    }

    it->second.response.headers.emplace_back(std::move(hname), std::move(hvalue));
    return 0;
}

int H2Session::on_data_chunk(nghttp2_session* /*session*/, uint8_t /*flags*/,
                              int32_t stream_id, const uint8_t* data,
                              size_t len, void* user_data) {
    auto* self = static_cast<H2Session*>(user_data);
    auto it = self->streams_.find(stream_id);
    if (it == self->streams_.end()) return 0;

    it->second.response.body.append(reinterpret_cast<const char*>(data), len);
    return 0;
}

int H2Session::on_stream_close(nghttp2_session* /*session*/, int32_t stream_id,
                                uint32_t /*error_code*/, void* user_data) {
    auto* self = static_cast<H2Session*>(user_data);
    auto it = self->streams_.find(stream_id);
    if (it != self->streams_.end()) {
        it->second.response.complete = true;
    }
    return 0;
}

int H2Session::on_frame_recv(nghttp2_session* /*session*/, const nghttp2_frame* /*frame*/,
                              void* /*user_data*/) {
    return 0;
}

ssize_t H2Session::on_data_source_read(nghttp2_session* /*session*/, int32_t stream_id,
                                         uint8_t* buf, size_t length,
                                         uint32_t* data_flags,
                                         nghttp2_data_source* /*source*/,
                                         void* user_data) {
    auto* self = static_cast<H2Session*>(user_data);
    auto it = self->streams_.find(stream_id);
    if (it == self->streams_.end()) {
        *data_flags |= NGHTTP2_DATA_FLAG_EOF;
        return 0;
    }

    auto& state = it->second;
    size_t remaining = state.body_len - state.body_offset;
    size_t to_copy = std::min(remaining, length);

    if (to_copy > 0) {
        std::memcpy(buf, static_cast<const uint8_t*>(state.body_data) + state.body_offset, to_copy);
        state.body_offset += to_copy;
    }

    if (state.body_offset >= state.body_len) {
        *data_flags |= NGHTTP2_DATA_FLAG_EOF;
    }

    return static_cast<ssize_t>(to_copy);
}

} // namespace rtt
