#pragma once

#include "abi.hpp"

#include <cstdint>
#include <memory>

namespace kvm_host {
class Bus;
class Kvm;

class KvmCpu {
public:
    KvmCpu() = default;

    KvmCpu(const KvmCpu &) = delete;
    KvmCpu &operator=(const KvmCpu &) = delete;

    KvmCpu(KvmCpu &&other) noexcept;
    KvmCpu &operator=(KvmCpu &&other) noexcept;

    ~KvmCpu();

    bool init(std::uintptr_t pc, std::uintptr_t a1, std::uintptr_t a0 = 0) const;
    bool run(std::uintptr_t *exit_reason) const;
    bool get_regs(KvmRegs &regs) const;
    bool get_sregs(KvmSRegs &regs) const;
    bool get_page_fault(KvmPageFault &page_fault) const;
    bool set_regs(const KvmRegs &regs) const;
    bool set_interrupt_pending(const KvmInterrupt &interrupt) const;
    bool clear_interrupt_pending(const KvmInterrupt &interrupt) const;
    std::uint8_t *translate_guest_vaddr(std::uintptr_t guest_vaddr, std::uintptr_t length) const;
    std::shared_ptr<Bus> bus() const;
    int raw_fd() const;

private:
    enum class SbiCallResult {
        Resume,
        Shutdown,
        Failed,
    };

    KvmCpu(int fd, std::shared_ptr<Bus> bus);

    friend class Kvm;

    bool run_once(std::uintptr_t *exit_reason) const;
    SbiCallResult handle_sbi_call(const KvmRegs &regs) const;
    bool handle_memory_fault() const;

    int fd_ = -1;
    std::shared_ptr<Bus> bus_;
};
} // namespace kvm_host
