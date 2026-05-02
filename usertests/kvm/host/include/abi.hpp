#pragma once

#include <cstddef>
#include <cstdint>

namespace kvm_host {
constexpr unsigned long KVM_CREATE_VCPU = 1;
constexpr unsigned long KVM_MAP_AREA = 2;
constexpr unsigned long KVM_RUN = 1;
constexpr unsigned long KVM_GET_REGS = 2;
constexpr unsigned long KVM_SET_REGS = 3;
constexpr unsigned long KVM_GET_SREGS = 4;
constexpr unsigned long KVM_GET_PAGE_FAULT = 5;
constexpr unsigned long KVM_SET_INTERRUPT_PENDING = 6;
constexpr unsigned long KVM_CLEAR_INTERRUPT_PENDING = 7;

constexpr std::uintptr_t KVM_INTERRUPT_HARDWARE = 2;

struct KvmMapArea {
    std::uintptr_t addr;
    std::uintptr_t length;
    std::uintptr_t mapped_addr;
};

enum class KvmReg : std::size_t {
    Pc = 0,
    A0 = 10,
    A1 = 11,
    A6 = 16,
    A7 = 17,
    Count = 32,
};

inline constexpr std::size_t kvm_reg_index(KvmReg reg) {
    return static_cast<std::size_t>(reg);
}

struct KvmRegs {
    std::uintptr_t regs[kvm_reg_index(KvmReg::Count)];

    std::uintptr_t &operator[](std::size_t index) {
        return this->regs[index];
    }

    const std::uintptr_t &operator[](std::size_t index) const {
        return this->regs[index];
    }

    std::uintptr_t &operator[](KvmReg reg) {
        return (*this)[kvm_reg_index(reg)];
    }

    const std::uintptr_t &operator[](KvmReg reg) const {
        return (*this)[kvm_reg_index(reg)];
    }
};

struct KvmSRegs {
    std::uintptr_t satp;
};

struct KvmPageFault {
    std::uintptr_t addr;
    std::uintptr_t access_type;
    std::uintptr_t inst;
};

struct KvmInterrupt {
    std::uintptr_t kind;
    std::uintptr_t irq;
};

static_assert(sizeof(KvmMapArea) == sizeof(std::uintptr_t) * 3, "KvmMapArea ABI changed");
static_assert(kvm_reg_index(KvmReg::Count) == 32, "KvmRegs register count changed");
static_assert(sizeof(KvmRegs) == sizeof(std::uintptr_t) * 32, "KvmRegs ABI changed");
static_assert(sizeof(KvmSRegs) == sizeof(std::uintptr_t), "KvmSRegs ABI changed");
static_assert(sizeof(KvmPageFault) == sizeof(std::uintptr_t) * 3, "KvmPageFault ABI changed");
static_assert(sizeof(KvmInterrupt) == sizeof(std::uintptr_t) * 2, "KvmInterrupt ABI changed");
} // namespace kvm_host
