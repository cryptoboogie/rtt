add_executable(rtt_tests
    test_sanity.cpp
    test_deps.cpp
    test_monotonic_clock.cpp
    test_timestamp_record.cpp
)
target_link_libraries(rtt_tests
    GTest::gtest_main
    rtt_lib
)

include(GoogleTest)
gtest_discover_tests(rtt_tests)
