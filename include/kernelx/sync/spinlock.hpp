#ifndef __KXERNELX_SYNC_SPINLOCK_H__
#define __KXERNELX_SYNC_SPINLOCK_H__

#include <atomic>

namespace kernelx::sync {

class SpinLock {
private:
    std::atomic<bool> flag;
public:
    SpinLock() : flag(false) {}

    void lock() {
        while (flag.exchange(true, std::memory_order_acquire)) {
            // Busy-wait
        }
    }

    void unlock() {
        flag.store(false, std::memory_order_release);
    }

    bool try_lock() {
        return !flag.exchange(true, std::memory_order_acquire);
    }
};

}

#endif
