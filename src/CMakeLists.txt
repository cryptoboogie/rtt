add_library(rtt_lib
    clock/monotonic_clock.cpp
    metrics/stats_aggregator.cpp
    request/request_template.cpp
    connection/tcp_connector.cpp
    connection/tls_session.cpp
    connection/h2_session.cpp
)
target_include_directories(rtt_lib PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(rtt_lib PUBLIC OpenSSL::SSL OpenSSL::Crypto nghttp2::nghttp2)

add_executable(rtt-executor main.cpp)
target_link_libraries(rtt-executor PRIVATE rtt_lib)
