#pragma once

#include <atomic>
#include <array>
#include <cstddef>
#include <new>
#include <optional>
#include <type_traits>

namespace rtt {

// Lock-free single-producer single-consumer ring buffer.
// Capacity must be a power of two.
// Cache-line padding prevents false sharing between head and tail.
template <typename T, size_t Capacity>
class SPSCQueue {
    static_assert((Capacity & (Capacity - 1)) == 0, "Capacity must be a power of two");
    static_assert(std::is_trivially_copyable_v<T>, "T must be trivially copyable");

public:
    SPSCQueue() : head_(0), tail_(0) {}

    // Producer: try to push an element. Returns false if full.
    bool push(const T& item) noexcept {
        size_t head = head_.load(std::memory_order_relaxed);
        size_t next = (head + 1) & mask_;
        if (next == tail_.load(std::memory_order_acquire)) {
            return false; // full
        }
        buffer_[head] = item;
        head_.store(next, std::memory_order_release);
        return true;
    }

    // Consumer: try to pop an element. Returns std::nullopt if empty.
    std::optional<T> pop() noexcept {
        size_t tail = tail_.load(std::memory_order_relaxed);
        if (tail == head_.load(std::memory_order_acquire)) {
            return std::nullopt; // empty
        }
        T item = buffer_[tail];
        tail_.store((tail + 1) & mask_, std::memory_order_release);
        return item;
    }

    bool empty() const noexcept {
        return head_.load(std::memory_order_acquire) == tail_.load(std::memory_order_acquire);
    }

    size_t capacity() const noexcept { return Capacity; }

private:
    static constexpr size_t mask_ = Capacity - 1;

    // Align to cache line to prevent false sharing
    static constexpr size_t cache_line_size = 64;

    alignas(cache_line_size) std::atomic<size_t> head_;
    alignas(cache_line_size) std::atomic<size_t> tail_;
    alignas(cache_line_size) std::array<T, Capacity> buffer_;
};

} // namespace rtt
