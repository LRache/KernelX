#pragma once

#include "kvm.hpp"

#include <cstdint>
#include <string>

namespace kvm_host {
class Bus;

inline constexpr std::uintptr_t DEFAULT_GUEST_MEMORY_SIZE = 0x08000000;
inline constexpr std::uintptr_t GUEST_KERNEL_LOAD_ADDR = 0x80200000;
inline constexpr std::uintptr_t GUEST_DTB_LOAD_ADDR = 0x82200000;
inline constexpr std::uintptr_t GUEST_INITRD_LOAD_ADDR = 0x84200000;

inline constexpr std::uintptr_t UART0_BASE = 0x10000000;
inline constexpr std::uintptr_t PLIC_BASE = 0x0c000000;
inline constexpr std::uint32_t UART0_IRQ = 10;

inline constexpr const char *DEFAULT_GUEST_IMAGE = "/guest/hello_sbi.bin";

struct GuestBootOptions {
    const char *kernel_path = DEFAULT_GUEST_IMAGE;
    const char *initrd_path = nullptr;
    const char *dtb_path = nullptr;
    std::string bootargs;
    std::uintptr_t memory_size = DEFAULT_GUEST_MEMORY_SIZE;
};

struct GuestEntry {
    std::uintptr_t pc = 0;
    std::uintptr_t a1 = 0;
};

bool parse_uintptr_arg(const char *text, std::uintptr_t *value);
void print_usage(const char *argv0);

bool prepare_guest(const Kvm &kvm, const GuestMapping &mapping, const GuestBootOptions &options, const Bus &bus,
                   GuestEntry *entry);
int boot_guest(const GuestMapping &mapping, const GuestEntry &entry, Kvm *kvm);
} // namespace kvm_host
