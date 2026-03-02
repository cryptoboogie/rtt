#pragma once

#include <openssl/ssl.h>
#include <string>
#include <cstddef>

namespace rtt {

// TLS session wrapper with ALPN h2 negotiation.
// Owns SSL* and SSL_CTX* (or shares CTX).
class TlsSession {
public:
    TlsSession();
    ~TlsSession();

    // Non-copyable
    TlsSession(const TlsSession&) = delete;
    TlsSession& operator=(const TlsSession&) = delete;

    // Movable
    TlsSession(TlsSession&& other) noexcept;
    TlsSession& operator=(TlsSession&& other) noexcept;

    // Perform TLS handshake over an existing socket fd.
    // Sets SNI and ALPN h2. Returns true on success.
    bool handshake(int fd, const std::string& hostname);

    // Get the negotiated ALPN protocol (e.g., "h2")
    std::string negotiated_protocol() const;

    // Get the negotiated TLS version string
    std::string tls_version() const;

    // Raw SSL read/write for use by H2 session
    int ssl_read(void* buf, size_t len);
    int ssl_write(const void* buf, size_t len);

    // Check if session is valid/connected
    bool is_valid() const { return ssl_ != nullptr; }

    // Get underlying SSL* (for advanced use)
    SSL* ssl() const { return ssl_; }

    // Shutdown and close
    void close();

private:
    SSL_CTX* ctx_ = nullptr;
    SSL* ssl_ = nullptr;

    static SSL_CTX* create_ctx();
};

} // namespace rtt
