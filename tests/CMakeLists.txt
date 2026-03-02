add_executable(rtt_tests test_sanity.cpp)
target_link_libraries(rtt_tests GTest::gtest_main)

include(GoogleTest)
gtest_discover_tests(rtt_tests)
