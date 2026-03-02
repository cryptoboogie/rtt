add_executable(rtt_tests
    test_sanity.cpp
    test_deps.cpp
)
target_link_libraries(rtt_tests
    GTest::gtest_main
    OpenSSL::SSL
    OpenSSL::Crypto
    nghttp2::nghttp2
)

include(GoogleTest)
gtest_discover_tests(rtt_tests)
