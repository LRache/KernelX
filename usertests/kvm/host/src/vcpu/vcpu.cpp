#include "vcpu.hpp"

#include "device/bus.hpp"

#include <cstdio>
#include <unistd.h>
#include <utility>

namespace kvm_host {

enum KvmExitReason : std::uintptr_t {
    MemoryAccessFault = 1,
    SbiCall = 16,
};

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
        reset_fd(&this->fd_, std::exchange(other.fd_, -1));
        this->bus_ = std::move(other.bus_);
    }
    return *this;
}

KvmCpu::~KvmCpu() {
    reset_fd(&this->fd_);
}

bool KvmCpu::run(std::uintptr_t *exit_reason) const {
    while (true) {
        if (this->bus_ != nullptr) {
            this->bus_->update();
        }

        std::uintptr_t reason = 0;
        if (!this->run_once(&reason)) {
            std::fprintf(stderr, "kvm vcpu run failed\n");
            return false;
        }
        if (exit_reason != nullptr) {
            *exit_reason = reason;
        }

        switch (reason) {
            case KvmExitReason::SbiCall: {
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
            case KvmExitReason::MemoryAccessFault:
                if (this->handle_memory_fault()) {
                    continue;
                }
                return false;
            default:
                std::fprintf(stderr, "unsupported kvm exit reason: 0x%lx\n", static_cast<unsigned long>(reason));
                return false;
        }
    }
}

std::shared_ptr<Bus> KvmCpu::bus() const {
    return this->bus_;
}

int KvmCpu::raw_fd() const {
    return this->fd_;
}
} // namespace kvm_host
