add_library(rtt_lib
    clock/monotonic_clock.cpp
)
target_include_directories(rtt_lib PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(rtt_lib PUBLIC OpenSSL::SSL OpenSSL::Crypto nghttp2::nghttp2)

add_executable(rtt-executor main.cpp)
target_link_libraries(rtt-executor PRIVATE rtt_lib)
