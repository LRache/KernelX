#include "vcpu.hpp"

#include <cstddef>
#include <cstdio>

namespace kvm_host {
enum class SbiExtensionId : std::uintptr_t {
    LegacySetTimer = 0x0,
    LegacyConsolePutchar = 0x1,
    LegacyConsoleGetchar = 0x2,
    LegacyShutdown = 0x8,
    Base = 0x10,
    Time = 0x54494d45,
    RemoteFence = 0x52464e43,
    SystemReset = 0x53525354,
    DebugConsole = 0x4442434e,
};

enum class SbiBaseFunctionId : std::uintptr_t {
    GetSpecVersion = 0,
    GetImplId = 1,
    GetImplVersion = 2,
    ProbeExtension = 3,
    GetMvendorId = 4,
    GetMarchId = 5,
    GetMimpId = 6,
};

enum class SbiDebugConsoleFunctionId : std::uintptr_t {
    ConsoleWrite = 0,
};

enum class SbiRemoteFenceFunctionId : std::uintptr_t {
    RemoteFenceI = 0,
    RemoteSfenceVma = 1,
    RemoteSfenceVmaAsid = 2,
};

enum class SbiSystemResetFunctionId : std::uintptr_t {
    Reset = 0,
};

static constexpr std::intptr_t SBI_SUCCESS = 0;
static constexpr std::intptr_t SBI_ERR_NOT_SUPPORTED = -2;
static constexpr std::intptr_t SBI_ERR_INVALID_ADDRESS = -5;

static constexpr std::uintptr_t SBI_SPEC_VERSION_0_3 = 3;
static constexpr std::uintptr_t SBI_IMPL_ID_OPENSBI = 1;

static constexpr std::uintptr_t sbi_extension_id_value(SbiExtensionId extension_id) {
    return static_cast<std::uintptr_t>(extension_id);
}

static bool finish_legacy_call(const KvmCpu &cpu, const KvmRegs &regs) {
    KvmRegs next = regs;
    next[KvmReg::Pc] += 4;
    return cpu.set_regs(next);
}

static bool finish_sbi_call(const KvmCpu &cpu, const KvmRegs &regs, std::intptr_t error, std::uintptr_t value) {
    KvmRegs next = regs;
    next[KvmReg::Pc] += 4;
    next[KvmReg::A0] = static_cast<std::uintptr_t>(error);
    next[KvmReg::A1] = value;
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
    switch (static_cast<SbiExtensionId>(extension_id)) {
        case SbiExtensionId::Base:
        case SbiExtensionId::Time:
        case SbiExtensionId::RemoteFence:
        case SbiExtensionId::SystemReset:
        case SbiExtensionId::DebugConsole:
        case SbiExtensionId::LegacySetTimer:
        case SbiExtensionId::LegacyConsolePutchar:
        case SbiExtensionId::LegacyConsoleGetchar:
        case SbiExtensionId::LegacyShutdown:
            return true;
        default:
            return false;
    }
}

static bool handle_base_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    std::uintptr_t value = 0;

    switch (static_cast<SbiBaseFunctionId>(regs[KvmReg::A6])) {
        case SbiBaseFunctionId::GetSpecVersion:
            value = SBI_SPEC_VERSION_0_3;
            break;
        case SbiBaseFunctionId::GetImplId:
            value = SBI_IMPL_ID_OPENSBI;
            break;
        case SbiBaseFunctionId::GetImplVersion:
            value = 0;
            break;
        case SbiBaseFunctionId::ProbeExtension:
            value = is_extension_supported(regs[KvmReg::A0]) ? 1 : 0;
            break;
        case SbiBaseFunctionId::GetMvendorId:
        case SbiBaseFunctionId::GetMarchId:
        case SbiBaseFunctionId::GetMimpId:
            value = 0;
            break;
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }

    return finish_sbi_call(cpu, regs, SBI_SUCCESS, value);
}

static bool handle_debug_console_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (static_cast<SbiDebugConsoleFunctionId>(regs[KvmReg::A6])) {
        case SbiDebugConsoleFunctionId::ConsoleWrite:
            if (!write_guest_buffer_to_stdout(cpu, regs[KvmReg::A1], regs[KvmReg::A0])) {
                return finish_sbi_call(cpu, regs, SBI_ERR_INVALID_ADDRESS, 0);
            }
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, regs[KvmReg::A0]);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

static bool handle_remote_fence_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (static_cast<SbiRemoteFenceFunctionId>(regs[KvmReg::A6])) {
        case SbiRemoteFenceFunctionId::RemoteFenceI:
        case SbiRemoteFenceFunctionId::RemoteSfenceVma:
        case SbiRemoteFenceFunctionId::RemoteSfenceVmaAsid:
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, 0);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

static bool handle_system_reset_extension(const KvmCpu &cpu, const KvmRegs &regs) {
    switch (static_cast<SbiSystemResetFunctionId>(regs[KvmReg::A6])) {
        case SbiSystemResetFunctionId::Reset:
            std::printf("kvm exit: riscv sbi system reset type=%lu reason=%lu\n",
                        static_cast<unsigned long>(regs[KvmReg::A0]), static_cast<unsigned long>(regs[KvmReg::A1]));
            return finish_sbi_call(cpu, regs, SBI_SUCCESS, 0);
        default:
            return finish_sbi_call(cpu, regs, SBI_ERR_NOT_SUPPORTED, 0);
    }
}

KvmCpu::SbiCallResult KvmCpu::handle_sbi_call(const KvmRegs &regs) const {
    switch (static_cast<SbiExtensionId>(regs[KvmReg::A7])) {
        case SbiExtensionId::LegacyConsolePutchar:
            std::fputc(static_cast<unsigned char>(regs[KvmReg::A0]), stdout);
            std::fflush(stdout);
            return finish_legacy_call(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SbiExtensionId::LegacyConsoleGetchar: {
            KvmRegs next = regs;
            next[KvmReg::Pc] += 4;
            next[KvmReg::A0] = static_cast<std::uintptr_t>(-1);
            return this->set_regs(next) ? SbiCallResult::Resume : SbiCallResult::Failed;
        }
        case SbiExtensionId::LegacyShutdown:
            std::printf("kvm exit: riscv sbi shutdown\n");
            std::fflush(stdout);
            return SbiCallResult::Shutdown;
        case SbiExtensionId::Base:
            return handle_base_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SbiExtensionId::DebugConsole:
            return handle_debug_console_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SbiExtensionId::RemoteFence:
            return handle_remote_fence_extension(*this, regs) ? SbiCallResult::Resume : SbiCallResult::Failed;
        case SbiExtensionId::SystemReset:
            if (!handle_system_reset_extension(*this, regs)) {
                return SbiCallResult::Failed;
            }
            return SbiCallResult::Shutdown;
        default:
            if (regs[KvmReg::A7] >= sbi_extension_id_value(SbiExtensionId::Base)) {
                return finish_sbi_call(*this, regs, SBI_ERR_NOT_SUPPORTED, 0) ? SbiCallResult::Resume
                                                                              : SbiCallResult::Failed;
            }

            std::fprintf(stderr, "unsupported riscv legacy sbi call: eid=0x%lx fid=0x%lx\n",
                         static_cast<unsigned long>(regs[KvmReg::A7]), static_cast<unsigned long>(regs[KvmReg::A6]));
            return SbiCallResult::Failed;
    }
}
} // namespace kvm_host
