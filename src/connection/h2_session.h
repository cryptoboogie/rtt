#pragma once

#include "connection/tls_session.h"
#include <nghttp2/nghttp2.h>
#include <string>
#include <vector>
#include <unordered_map>
#include <cstdint>
#include <functional>
#include <span>

namespace rtt {

struct H2Response {
    int32_t stream_id = 0;
    int status_code = 0;
    std::vector<std::pair<std::string, std::string>> headers;
    std::string body;
    bool complete = false;

    std::string get_header(const std::string& name) const;
};

// HTTP/2 client session over a TLS connection using nghttp2.
class H2Session {
public:
    H2Session();
    ~H2Session();

    // Non-copyable, movable
    H2Session(const H2Session&) = delete;
    H2Session& operator=(const H2Session&) = delete;
    H2Session(H2Session&& other) noexcept;
    H2Session& operator=(H2Session&& other) noexcept;

    // Initialize session over an existing TLS connection.
    // Takes ownership of the TlsSession.
    bool init(TlsSession&& tls);

    // Submit a request with prebuilt nghttp2_nv headers.
    // Returns stream ID or -1 on error.
    int32_t submit_request(std::span<nghttp2_nv> headers,
                           const void* body_data = nullptr,
                           size_t body_len = 0);

    // Run the I/O loop until the given stream's response is complete.
    // Returns the response, or an incomplete response on error/timeout.
    H2Response run_until_response(int32_t stream_id);

    // Send a PING frame (for keepalive)
    bool send_ping();

    // Check if session is valid
    bool is_valid() const { return session_ != nullptr; }

    // Get the last cf-ray header value observed
    const std::string& last_cf_ray() const { return last_cf_ray_; }

private:
    nghttp2_session* session_ = nullptr;
    TlsSession tls_;
    std::string last_cf_ray_;

    // Per-stream state
    struct StreamState {
        H2Response response;
        const void* body_data = nullptr;
        size_t body_len = 0;
        size_t body_offset = 0;
    };
    std::unordered_map<int32_t, StreamState> streams_;

    // I/O helpers
    bool send_pending();
    bool recv_data();

    // Cleanup
    void destroy();

    // nghttp2 callbacks
    static ssize_t on_send(nghttp2_session* session, const uint8_t* data,
                           size_t length, int flags, void* user_data);
    static int on_header(nghttp2_session* session, const nghttp2_frame* frame,
                         const uint8_t* name, size_t namelen,
                         const uint8_t* value, size_t valuelen,
                         uint8_t flags, void* user_data);
    static int on_data_chunk(nghttp2_session* session, uint8_t flags,
                             int32_t stream_id, const uint8_t* data,
                             size_t len, void* user_data);
    static int on_stream_close(nghttp2_session* session, int32_t stream_id,
                               uint32_t error_code, void* user_data);
    static int on_frame_recv(nghttp2_session* session, const nghttp2_frame* frame,
                             void* user_data);
    static ssize_t on_data_source_read(nghttp2_session* session, int32_t stream_id,
                                        uint8_t* buf, size_t length,
                                        uint32_t* data_flags,
                                        nghttp2_data_source* source,
                                        void* user_data);
};

} // namespace rtt
