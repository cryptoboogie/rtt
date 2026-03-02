#pragma once

#include <cstddef>

namespace rtt {

// Pin the calling thread to a specific CPU core.
// Returns true on success, false on failure or if not supported.
bool pin_to_core(size_t core_id);

} // namespace rtt
