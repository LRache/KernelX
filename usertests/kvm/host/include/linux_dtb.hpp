#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace kvm_host {
struct LinuxGuestDtbConfig {
    std::uintptr_t memory_base = 0;
    std::uintptr_t memory_size = 0;
    std::uintptr_t uart_base = 0;
    std::uintptr_t uart_size = 0;
    std::uintptr_t plic_base = 0;
    std::uintptr_t plic_size = 0;
    std::uint32_t uart_clock_hz = 3686400;
    std::uint32_t uart_baud = 115200;
    std::uint32_t uart_irq = 0;
    std::uint32_t timebase_frequency = 10000000;
    std::uint32_t cpu_intc_phandle = 1;
    std::uint32_t plic_phandle = 2;
    std::uint32_t plic_ndev = 0;
    std::uintptr_t initrd_start = 0;
    std::uintptr_t initrd_end = 0;
    bool has_initrd = false;
    bool has_plic = false;
    std::string bootargs;
    std::string stdout_path;
    std::string riscv_isa = "rv64imafd_zicsr_zifencei";
    std::string mmu_type = "riscv,sv39";
};

std::vector<std::uint8_t> build_linux_guest_dtb(const LinuxGuestDtbConfig &config);
} // namespace kvm_host
