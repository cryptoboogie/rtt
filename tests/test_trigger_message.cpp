#include <gtest/gtest.h>
#include "trigger/trigger_message.h"
#include <cstring>

using namespace rtt;

TEST(TriggerMessage, SizeIsFixed) {
    // Size should be constant and known at compile time
    constexpr size_t expected = sizeof(uint64_t)    // trigger_id
                              + sizeof(uint64_t)    // t_trigger_rx
                              + sizeof(uint32_t)    // action
                              + sizeof(uint32_t)    // reserved
                              + 64;                 // payload
    EXPECT_EQ(sizeof(TriggerMessage), expected);
}

TEST(TriggerMessage, DefaultConstruction) {
    TriggerMessage msg{};
    EXPECT_EQ(msg.trigger_id, 0u);
    EXPECT_EQ(msg.t_trigger_rx, 0u);
    EXPECT_EQ(msg.action, ActionType::EXECUTE_YES);
}

TEST(TriggerMessage, CreateFactory) {
    auto msg = TriggerMessage::create(42, ActionType::EXECUTE_YES);
    EXPECT_EQ(msg.trigger_id, 42u);
    EXPECT_EQ(msg.action, ActionType::EXECUTE_YES);
}

TEST(TriggerMessage, RoundTrip) {
    auto original = TriggerMessage::create(123);
    original.t_trigger_rx = 999'888'777;
    const char* test_payload = "MARKET_ABC";
    original.set_payload(test_payload, strlen(test_payload));

    // Copy (simulating SPSC queue transfer)
    TriggerMessage copy;
    std::memcpy(&copy, &original, sizeof(TriggerMessage));

    EXPECT_EQ(copy.trigger_id, 123u);
    EXPECT_EQ(copy.t_trigger_rx, 999'888'777u);
    EXPECT_EQ(copy.action, ActionType::EXECUTE_YES);
    EXPECT_EQ(std::memcmp(copy.payload, test_payload, strlen(test_payload)), 0);
}

TEST(TriggerMessage, PayloadTruncation) {
    TriggerMessage msg{};
    char large[128];
    std::memset(large, 'X', sizeof(large));
    msg.set_payload(large, sizeof(large));
    // Should copy only 64 bytes (payload size), not overflow
    EXPECT_EQ(msg.payload[63], 'X');
}
