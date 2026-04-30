#include "vcpu.hpp"

#include <cstddef>
#include <cstdio>

namespace kvm_host {
static constexpr std::uintptr_t RISCV_SBI_LEGACY_SET_TIMER_EID = 0x0;
static constexpr std::uintptr_t RISCV_SBI_LEGACY_CONSOLE_PUTCHAR_EID = 0x1;
static constexpr std::uintptr_t RISCV_SBI_LEGACY_CONSOLE_GETCHAR_EID = 0x2;
static constexpr std::uintptr_t RISCV_SBI_LEGACY_SHUTDOWN_EID = 0x8;

static constexpr std::uintptr_t SBI_EXT_BASE = 0x10;
static constexpr std::uintptr_t SBI_EXT_TIME = 0x54494d45;
static constexpr std::uintptr_t SBI_EXT_SRST = 0x53525354;
static constexpr std::uintptr_t SBI_EXT_DBCN = 0x4442434e;

static constexpr std::uintptr_t SBI_BASE_GET_SPEC_VERSION = 0;
static constexpr std::uintptr_t SBI_BASE_GET_IMPL_ID = 1;
static constexpr std::uintptr_t SBI_BASE_GET_IMPL_VERSION = 2;
static constexpr std::uintptr_t SBI_BASE_PROBE_EXTENSION = 3;
static constexpr std::uintptr_t SBI_BASE_GET_MVENDORID = 4;
static constexpr std::uintptr_t SBI_BASE_GET_MARCHID = 5;
static constexpr std::uintptr_t SBI_BASE_GET_MIMPID = 6;

static constexpr std::uintptr_t SBI_TIME_SET_TIMER = 0;
static constexpr std::uintptr_t SBI_DBCN_CONSOLE_WRITE = 0;
static constexpr std::uintptr_t SBI_SRST_RESET = 0;

static constexpr std::intptr_t SBI_SUCCESS = 0;
static constexpr std::intptr_t SBI_ERR_NOT_SUPPORTED = -2;
static constexpr std::intptr_t SBI_ERR_INVALID_ADDRESS = -5;

static constexpr std::uintptr_t SBI_SPEC_VERSION_0_3 = 3;
static constexpr std::uintptr_t SBI_IMPL_ID_OPENSBI = 1;

static bool finish_legacy_call(const KvmCpu &cpu, const KvmRegs &regs) {
    KvmRegs next = regs;
    next.pc += 4;
    return cpu.set_regs(next);
}

static bool finish_sbi_call(const KvmCpu &cpu, const KvmRegs &regs, std::intptr_t error, std::uintptr_t value) {
    KvmRegs next = regs;
    next.pc += 4;
    next.a0 = static_cast<std::uintptr_t>(error);
    next.a1 = value;
    return cpu.set_regs(next);
}

static bool write_guest_buffer_to_stdout(const KvmCpu &cpu, std::uintptr_t guest_addr, std::uintptr_t length) {
    static constexpr std::uintptr_t PAGE_SIZE = 4096;

    while (length != 0) {
        const std::uintptr_t page_remaining = PAGE_SIZE - (guest_addr & (PAGE_SIZE - 1));
        const std::uintptr_t chunk = length < page_remaining ? length : page_remaining;
        std::uint8_t *host_addr = cpu.translate_guest_vaddr(guest_addr, chunk);
        if (host_addr == nullptr) {
            return false;
        }

        if (std::fwrite(host_addr, 1, static_cast<std::size_t>(chunk), stdout) != chunk) {
            return false;
        }

        guest_addr += chunk;
        length -= chunk;
    }

    std::fflush(stdout);
    return true;
}

static bool is_extension_supported(std::uintptr_t extension_id) {
    switch (extension_id) {
        case SBI_EXT_BASE:
        case SBI_EXT_TIME:
        case SBI_EXT_SRST:
        case SBI_EXT_DBCN:
        case RISCV_SBI_LEGACY_SET_TIMER_EID:
        case RISCV_SBI_LEGACY_CONSOLE_PUTCHAR_EID:
        case RISCV_SBI_LEGACY_CONSOLE_GETCHAR_EID:
        case RISCV_SBI_LEGACY_SHUTDOWN_EID:
            return true;
        default:
            return false;
    }
}

static bool handle_base_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    std::uintptr_t value = 0;

    switch (regs.a6) {
        case SBI_BASE_GET_SPEC_VERSION:
            value = SBI_SPEC_VERSION_0_3;
            break;
        case SBI_BASE_GET_IMPL_ID:
            value = SBI_IMPL_ID_OPENSBI;
            break;
        case SBI_BASE_GET_IMPL_VERSION:
            value = 0;
            break;
        case SBI_BASE_PROBE_EXTENSION:
            value = is_extension_supported(regs.a0) ? 1 : 0;
            break;
        case SBI_BASE_GET_MVENDORID:
        case SBI_BASE_GET_MARCHID:
        case SBI_BASE_GET_MIMPID:
            value = 0;
            break;
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }

    return finish_sbi_call(cpu, regs, SBI_SUCCESS, value);
}

static bool handle_time_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (regs.a6) {
        case SBI_TIME_SET_TIMER:
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, 0);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

static bool handle_debug_console_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (regs.a6) {
        case SBI_DBCN_CONSOLE_WRITE:
            if (!write_guest_buffer_to_stdout(cpu, regs.a1, regs.a0)) {
                return finish_sbi_call(cpu, regs, SBI_ERR_INVALID_ADDRESS, 0);
            }
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, regs.a0);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

static bool handle_system_reset_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (regs.a6) {
        case SBI_SRST_RESET:
            std::printf("kvm exit: riscv sbi system reset type=%lu reason=%lu\n",
                        static_cast<unsigned long>(regs.a0), static_cast<unsigned long>(regs.a1));
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, 0);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

KvmCpu::SbiCallResult KvmCpu::handle_sbi_call(const KvmRegs &regs) const {
    switch (regs.a7) {
        case RISCV_SBI_LEGACY_SET_TIMER_EID:
            return finish_legacy_call(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case RISCV_SBI_LEGACY_CONSOLE_PUTCHAR_EID:
            std::fputc(static_cast<unsigned char>(regs.a0), stdout);
            std::fflush(stdout);
            return finish_legacy_call(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case RISCV_SBI_LEGACY_CONSOLE_GETCHAR_EID: {
            KvmRegs next = regs;
            next.pc += 4;
            next.a0 = static_cast<std::uintptr_t>(-1);
            return set_regs(next) ? SbiCallResult::Resume : SbiCallResult::Failed;
        }
        case RISCV_SBI_LEGACY_SHUTDOWN_EID:
            std::printf("kvm exit: riscv sbi shutdown\n");
            return SbiCallResult::Shutdown;
        case SBI_EXT_BASE:
            return handle_base_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SBI_EXT_TIME:
            return handle_time_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SBI_EXT_DBCN:
            return handle_debug_console_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SBI_EXT_SRST:
            if (!handle_system_reset_extension(*this, regs)) {
                return SbiCallResult::Failed;
            }
            return SbiCallResult::Shutdown;
        default:
            if (regs.a7 >= SBI_EXT_BASE) {
                return finish_sbi_call(*this, regs, SBI_ERR_NOT_SUPPORTED, 0) ? SbiCallResult::Resume
                                                                              : SbiCallResult::Failed;
            }

            std::fprintf(stderr, "unsupported riscv legacy sbi call: eid=0x%lx fid=0x%lx\n",
                         static_cast<unsigned long>(regs.a7), static_cast<unsigned long>(regs.a6));
            return SbiCallResult::Failed;
    }
}
} // namespace kvm_host
