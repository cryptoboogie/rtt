#include "connection/tls_session.h"

#include <openssl/err.h>
#include <cstring>

namespace rtt {

SSL_CTX* TlsSession::create_ctx() {
    const SSL_METHOD* method = TLS_client_method();
    SSL_CTX* ctx = SSL_CTX_new(method);
    if (!ctx) return nullptr;

    // Set minimum TLS version to 1.2, prefer 1.3
    SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);

    // Load system CA certificates
    SSL_CTX_set_default_verify_paths(ctx);
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, nullptr);

    // Set ALPN to negotiate h2
    static const unsigned char alpn[] = {2, 'h', '2'};
    SSL_CTX_set_alpn_protos(ctx, alpn, sizeof(alpn));

    return ctx;
}

TlsSession::TlsSession() : ctx_(create_ctx()) {}

TlsSession::~TlsSession() {
    close();
    if (ctx_) {
        SSL_CTX_free(ctx_);
        ctx_ = nullptr;
    }
}

TlsSession::TlsSession(TlsSession&& other) noexcept
    : ctx_(other.ctx_), ssl_(other.ssl_) {
    other.ctx_ = nullptr;
    other.ssl_ = nullptr;
}

TlsSession& TlsSession::operator=(TlsSession&& other) noexcept {
    if (this != &other) {
        close();
        if (ctx_) SSL_CTX_free(ctx_);
        ctx_ = other.ctx_;
        ssl_ = other.ssl_;
        other.ctx_ = nullptr;
        other.ssl_ = nullptr;
    }
    return *this;
}

bool TlsSession::handshake(int fd, const std::string& hostname) {
    if (!ctx_) return false;

    ssl_ = SSL_new(ctx_);
    if (!ssl_) return false;

    // Set SNI
    SSL_set_tlsext_host_name(ssl_, hostname.c_str());

    // Attach socket
    if (SSL_set_fd(ssl_, fd) != 1) {
        SSL_free(ssl_);
        ssl_ = nullptr;
        return false;
    }

    // Perform handshake
    int ret = SSL_connect(ssl_);
    if (ret != 1) {
        SSL_free(ssl_);
        ssl_ = nullptr;
        return false;
    }

    return true;
}

std::string TlsSession::negotiated_protocol() const {
    if (!ssl_) return "";

    const unsigned char* proto = nullptr;
    unsigned int proto_len = 0;
    SSL_get0_alpn_selected(ssl_, &proto, &proto_len);

    if (proto && proto_len > 0) {
        return std::string(reinterpret_cast<const char*>(proto), proto_len);
    }
    return "";
}

std::string TlsSession::tls_version() const {
    if (!ssl_) return "";
    return SSL_get_version(ssl_);
}

int TlsSession::ssl_read(void* buf, size_t len) {
    if (!ssl_) return -1;
    return SSL_read(ssl_, buf, static_cast<int>(len));
}

int TlsSession::ssl_write(const void* buf, size_t len) {
    if (!ssl_) return -1;
    return SSL_write(ssl_, buf, static_cast<int>(len));
}

void TlsSession::close() {
    if (ssl_) {
        SSL_shutdown(ssl_);
        SSL_free(ssl_);
        ssl_ = nullptr;
    }
}

} // namespace rtt
