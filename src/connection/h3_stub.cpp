#include "connection/h3_stub.h"
#include "connection/tcp_connector.h"
#include <unistd.h>
#include "connection/tls_session.h"
#include "connection/h2_session.h"
#include <nghttp2/nghttp2.h>

namespace rtt {

H3ProbeResult probe_h3_support(const std::string& hostname, uint16_t port) {
    H3ProbeResult result;

    // Connect via H2 and check for alt-svc header
    int fd = TcpConnector::connect_to_host(hostname, port);
    if (fd < 0) return result;

    TlsSession tls;
    if (!tls.handshake(fd, hostname)) {
        close(fd);
        return result;
    }

    H2Session session;
    if (!session.init(std::move(tls))) {
        return result;
    }

    // Send a simple GET request
    nghttp2_nv headers[] = {
        {(uint8_t*)":method", (uint8_t*)"GET", 7, 3, NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE},
        {(uint8_t*)":path", (uint8_t*)"/", 5, 1, NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE},
        {(uint8_t*)":scheme", (uint8_t*)"https", 7, 5, NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE},
        {(uint8_t*)":authority", (uint8_t*)hostname.c_str(), 10, hostname.size(), NGHTTP2_NV_FLAG_NO_COPY_NAME},
    };

    auto hdr_span = std::span<nghttp2_nv>(headers, 4);
    int32_t stream_id = session.submit_request(hdr_span, nullptr, 0);
    if (stream_id < 0) return result;

    auto response = session.run_until_response(stream_id);

    // Check for alt-svc header advertising h3
    std::string alt_svc = response.get_header("alt-svc");
    if (!alt_svc.empty()) {
        result.alt_svc_value = alt_svc;
        // Check if h3 is advertised (e.g., h3=":443")
        if (alt_svc.find("h3=") != std::string::npos ||
            alt_svc.find("h3-") != std::string::npos) {
            result.alt_svc_advertised = true;
        }
    }

    return result;
}

} // namespace rtt
