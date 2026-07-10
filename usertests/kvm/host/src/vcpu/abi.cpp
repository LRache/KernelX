#include "vcpu.hpp"

#include <cerrno>
#include <cstdio>
#include <sys/ioctl.h>

namespace kvm_host {
bool KvmCpu::run_once(std::uintptr_t *exit_reason) const {
    const long ret = ioctl(this->fd_, KVM_RUN, 0);
    if (ret < 0) {
        perror("ioctl(KVM_RUN)");
        return false;
    }

    if (exit_reason != nullptr) {
        *exit_reason = static_cast<std::uintptr_t>(ret);
    }
    return true;
}

bool KvmCpu::get_regs(KvmRegs &regs) const {
    regs = {};
    const long ret = ioctl(this->fd_, KVM_GET_REGS, &regs);
    if (ret < 0) {
        perror("ioctl(KVM_GET_REGS)");
        return false;
    }
    return true;
}

bool KvmCpu::get_sregs(KvmSRegs &regs) const {
    regs = {};
    const long ret = ioctl(this->fd_, KVM_GET_SREGS, &regs);
    if (ret < 0) {
        perror("ioctl(KVM_GET_SREGS)");
        return false;
    }
    return true;
}

bool KvmCpu::get_page_fault(KvmPageFault &page_fault) const {
    page_fault = {};
    const long ret = ioctl(this->fd_, KVM_GET_PAGE_FAULT, &page_fault);
    if (ret < 0) {
        perror("ioctl(KVM_GET_PAGE_FAULT)");
        return false;
    }
    return true;
}

bool KvmCpu::set_regs(const KvmRegs &regs) const {
    void *arg = const_cast<KvmRegs *>(&regs);
    const long ret = ioctl(this->fd_, KVM_SET_REGS, arg);
    if (ret < 0) {
        perror("ioctl(KVM_SET_REGS)");
        return false;
    }
    return true;
}

bool KvmCpu::init(std::uintptr_t pc, std::uintptr_t a1, std::uintptr_t a0) const {
    KvmRegs regs = {};
    regs[KvmReg::Pc] = pc;
    regs[KvmReg::A0] = a0;
    regs[KvmReg::A1] = a1;
    return this->set_regs(regs);
}

bool KvmCpu::set_interrupt_pending(const KvmInterrupt &interrupt) const {
    void *arg = const_cast<KvmInterrupt *>(&interrupt);
    const long ret = ioctl(this->fd_, KVM_SET_INTERRUPT_PENDING, arg);
    if (ret < 0) {
        perror("ioctl(KVM_SET_INTERRUPT_PENDING)");
        return false;
    }
    return true;
}

bool KvmCpu::clear_interrupt_pending(const KvmInterrupt &interrupt) const {
    void *arg = const_cast<KvmInterrupt *>(&interrupt);
    const long ret = ioctl(this->fd_, KVM_CLEAR_INTERRUPT_PENDING, arg);
    if (ret < 0) {
        perror("ioctl(KVM_CLEAR_INTERRUPT_PENDING)");
        return false;
    }
    return true;
}
} // namespace kvm_host
