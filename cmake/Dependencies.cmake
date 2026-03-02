# OpenSSL - use Homebrew on macOS (system LibreSSL lacks ALPN support)
if(APPLE)
    set(OPENSSL_ROOT_DIR "/opt/homebrew/opt/openssl@3" CACHE PATH "OpenSSL root")
endif()
find_package(OpenSSL REQUIRED)

# nghttp2 - HTTP/2 framing library
if(APPLE)
    set(NGHTTP2_PREFIX "/opt/homebrew/opt/libnghttp2")
    set(NGHTTP2_INCLUDE_DIRS "${NGHTTP2_PREFIX}/include")
    find_library(NGHTTP2_LIBRARY NAMES nghttp2 PATHS "${NGHTTP2_PREFIX}/lib" "/opt/homebrew/lib" NO_DEFAULT_PATH)
else()
    find_library(NGHTTP2_LIBRARY NAMES nghttp2)
    find_path(NGHTTP2_INCLUDE_DIRS nghttp2/nghttp2.h)
endif()

if(NOT NGHTTP2_LIBRARY)
    message(FATAL_ERROR "nghttp2 library not found")
endif()

add_library(nghttp2::nghttp2 UNKNOWN IMPORTED)
set_target_properties(nghttp2::nghttp2 PROPERTIES
    IMPORTED_LOCATION "${NGHTTP2_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${NGHTTP2_INCLUDE_DIRS}"
)

message(STATUS "OpenSSL version: ${OPENSSL_VERSION}")
message(STATUS "OpenSSL libraries: ${OPENSSL_LIBRARIES}")
message(STATUS "nghttp2 library: ${NGHTTP2_LIBRARY}")
