#include "vcpu.hpp"

#include "device/bus.hpp"

#include <cstdio>
#include <memory>
#include <unistd.h>
#include <utility>

namespace kvm_host {
static constexpr std::uintptr_t KVM_EXIT_SBI_CALL = 16;
static constexpr std::uintptr_t KVM_MEMORY_FAULT_ACCESS_MASK = 0x3;

static const char *memory_access_name(std::uintptr_t access_type) {
    switch (access_type) {
        case 0:
            return "read";
        case 1:
            return "write";
        case 2:
            return "execute";
        default:
            return "unknown";
    }
}

static bool decode_memory_fault(std::uintptr_t reason, std::uintptr_t *fault_addr, std::uintptr_t *access_type) {
    if (reason == KVM_EXIT_SBI_CALL || (reason & ~KVM_MEMORY_FAULT_ACCESS_MASK) == 0) {
        return false;
    }

    *fault_addr = reason >> 2;
    *access_type = reason & KVM_MEMORY_FAULT_ACCESS_MASK;
    return true;
}

static void reset_fd(int *slot, int fd = -1) {
    if (*slot >= 0) {
        close(*slot);
    }
    *slot = fd;
}

KvmCpu::KvmCpu(int fd, std::shared_ptr<Bus> bus) : fd_(fd), bus_(std::move(bus)) {}

KvmCpu::KvmCpu(KvmCpu &&other) noexcept : fd_(std::exchange(other.fd_, -1)), bus_(std::move(other.bus_)) {}

KvmCpu &KvmCpu::operator=(KvmCpu &&other) noexcept {
    if (this != &other) {
        reset_fd(&fd_, std::exchange(other.fd_, -1));
        bus_ = std::move(other.bus_);
    }
    return *this;
}

KvmCpu::~KvmCpu() {
    reset_fd(&fd_);
}

bool KvmCpu::run(std::uintptr_t *exit_reason) const {
    while (true) {
        if (bus_ != nullptr) {
            bus_->update();
        }

        std::uintptr_t reason = 0;
        if (!run_once(&reason)) {
            return false;
        }
        if (exit_reason != nullptr) {
            *exit_reason = reason;
        }

        switch (reason) {
            case KVM_EXIT_SBI_CALL: {
                KvmRegs regs = {};
                if (!this->get_regs(regs)) {
                    return false;
                }
                switch (this->handle_sbi_call(regs)) {
                    case SbiCallResult::Resume:
                        continue;
                    case SbiCallResult::Shutdown:
                        return true;
                    case SbiCallResult::Failed:
                        return false;
                };
                break;
            }
            default:
                std::uintptr_t fault_addr = 0;
                std::uintptr_t access_type = 0;
                if (decode_memory_fault(reason, &fault_addr, &access_type)) {
                    if (this->handle_mmio_fault(fault_addr, access_type)) {
                        continue;
                    }

                    std::fprintf(stderr, "unsupported kvm memory fault: addr=0x%lx access=%s\n",
                                 static_cast<unsigned long>(fault_addr), memory_access_name(access_type));
                    return false;
                }
                std::fprintf(stderr, "unsupported kvm exit reason: 0x%lx\n", static_cast<unsigned long>(reason));
                return false;
        }
    }
}

std::shared_ptr<Bus> KvmCpu::bus() const {
    return bus_;
}

int KvmCpu::raw_fd() const {
    return fd_;
}
} // namespace kvm_host
