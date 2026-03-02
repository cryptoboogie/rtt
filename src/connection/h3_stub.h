#pragma once

#include <string>
#include <cstdint>

namespace rtt {

// HTTP/3 experiment stub.
// This is a placeholder for future QUIC/H3 integration.
// The endpoint advertises h3 via alt-svc, but implementation
// requires a QUIC library (quiche, ngtcp2, msquic).
//
// When implemented, this should provide the same interface as H2Session:
// - init(hostname, port)
// - submit_request(headers, body, body_len)
// - run_until_response(stream_id)

enum class H3Status {
    NotImplemented,  // QUIC library not yet integrated
    Available,       // H3 ready for benchmarking
};

struct H3ProbeResult {
    bool alt_svc_advertised = false;  // Endpoint advertises h3
    std::string alt_svc_value;        // Raw alt-svc header value
};

// Probe the target endpoint for HTTP/3 support via an HTTP/2 request.
// Checks for alt-svc: h3=":443" in response headers.
H3ProbeResult probe_h3_support(const std::string& hostname, uint16_t port);

// Get the current H3 implementation status.
inline H3Status h3_status() { return H3Status::NotImplemented; }

} // namespace rtt
