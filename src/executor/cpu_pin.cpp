#include "executor/cpu_pin.h"

#if defined(__linux__)
#include <pthread.h>
#include <sched.h>
#endif

namespace rtt {

bool pin_to_core(size_t core_id) {
#if defined(__linux__)
    cpu_set_t cpuset;
    CPU_ZERO(&cpuset);
    CPU_SET(core_id, &cpuset);
    int rc = pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
    return rc == 0;
#else
    // macOS does not support thread affinity pinning.
    // This is a no-op; production deployment is on Linux.
    (void)core_id;
    return false;
#endif
}

} // namespace rtt
