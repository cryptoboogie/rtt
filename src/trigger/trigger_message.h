#pragma once

#include <cstdint>
#include <cstring>

namespace rtt {

enum class ActionType : uint32_t {
    EXECUTE_YES = 1,
};

// Fixed-size binary trigger message for zero-copy delivery via SPSC queue.
// All fields are plain data — no pointers, no virtuals, no heap.
struct TriggerMessage {
    uint64_t trigger_id  = 0;
    uint64_t t_trigger_rx = 0;       // Nanosecond timestamp (set by ingress)
    ActionType action    = ActionType::EXECUTE_YES;
    uint32_t reserved    = 0;        // Padding for alignment
    char payload[64]     = {};       // Reserved for market ID, price, size, etc.

    static TriggerMessage create(uint64_t id, ActionType action = ActionType::EXECUTE_YES) {
        TriggerMessage msg;
        msg.trigger_id = id;
        msg.action = action;
        return msg;
    }

    void set_payload(const void* data, size_t len) {
        size_t copy_len = len < sizeof(payload) ? len : sizeof(payload);
        std::memcpy(payload, data, copy_len);
    }
};

// Verify trivial copyability for SPSC queue compatibility
static_assert(std::is_trivially_copyable_v<TriggerMessage>,
              "TriggerMessage must be trivially copyable for SPSC queue");

} // namespace rtt
