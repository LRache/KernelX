#pragma once

#include <cstdint>

namespace kvm_host {
constexpr unsigned long KVM_CREATE_VCPU = 1;
constexpr unsigned long KVM_MAP_AREA = 2;
constexpr unsigned long KVM_RUN = 1;
constexpr unsigned long KVM_GET_REGS = 2;
constexpr unsigned long KVM_SET_REGS = 3;
constexpr unsigned long KVM_GET_SREGS = 4;
constexpr unsigned long KVM_GET_PAGE_FAULT = 5;

struct KvmMapArea {
    std::uintptr_t addr;
    std::uintptr_t length;
    std::uintptr_t mapped_addr;
};

struct KvmRegs {
    std::uintptr_t pc;
    std::uintptr_t ra;
    std::uintptr_t sp;
    std::uintptr_t gp;
    std::uintptr_t tp;
    std::uintptr_t t0;
    std::uintptr_t t1;
    std::uintptr_t t2;
    std::uintptr_t s0;
    std::uintptr_t s1;
    std::uintptr_t a0;
    std::uintptr_t a1;
    std::uintptr_t a2;
    std::uintptr_t a3;
    std::uintptr_t a4;
    std::uintptr_t a5;
    std::uintptr_t a6;
    std::uintptr_t a7;
    std::uintptr_t s2;
    std::uintptr_t s3;
    std::uintptr_t s4;
    std::uintptr_t s5;
    std::uintptr_t s6;
    std::uintptr_t s7;
    std::uintptr_t s8;
    std::uintptr_t s9;
    std::uintptr_t s10;
    std::uintptr_t s11;
    std::uintptr_t t3;
    std::uintptr_t t4;
    std::uintptr_t t5;
    std::uintptr_t t6;
};

struct KvmSRegs {
    std::uintptr_t satp;
};

struct KvmPageFault {
    std::uintptr_t addr;
    std::uintptr_t access_type;
    std::uintptr_t inst;
};

static_assert(sizeof(KvmMapArea) == sizeof(std::uintptr_t) * 3, "KvmMapArea ABI changed");
static_assert(sizeof(KvmRegs) == sizeof(std::uintptr_t) * 32, "KvmRegs ABI changed");
static_assert(sizeof(KvmSRegs) == sizeof(std::uintptr_t), "KvmSRegs ABI changed");
static_assert(sizeof(KvmPageFault) == sizeof(std::uintptr_t) * 3, "KvmPageFault ABI changed");
} // namespace kvm_host
