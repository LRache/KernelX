#include "vcpu.hpp"

#include "device/bus.hpp"

#include <cstddef>
#include <cstdint>
#include <cstdio>

namespace kvm_host {
enum class MemoryFaultAccess : std::uintptr_t {
    Read = 0,
    Write = 1,
    Execute = 2,
};

enum class Opcode : std::uintptr_t {
    Load = 0x03,
    Store = 0x23,
};

enum class LoadFunct3 : std::uintptr_t {
    Lb = 0,
    Lh = 1,
    Lw = 2,
    Ld = 3,
    Lbu = 4,
    Lhu = 5,
    Lwu = 6,
};

enum class StoreFunct3 : std::uintptr_t {
    Sb = 0,
    Sh = 1,
    Sw = 2,
    Sd = 3,
};

enum class AccessWidth : std::size_t {
    Byte = 1,
    Halfword = 2,
    Word = 4,
    Doubleword = 8,
};

static constexpr std::uintptr_t OPCODE_MASK = 0x7f;
static constexpr std::uintptr_t FUNCT3_MASK = 0x7;
static constexpr std::uintptr_t REG_INDEX_MASK = 0x1f;
static constexpr std::uintptr_t UNCOMPRESSED_INST_LOW_BITS = 0x3;
static constexpr std::uintptr_t ILLEGAL_COMPRESSED_INST_LOW_BIT = 0x1;
static constexpr std::uintptr_t INSTRUCTION_16BIT_SIZE = 2;
static constexpr std::uintptr_t INSTRUCTION_32BIT_SIZE = 4;

struct DecodedPageFaultInst {
    bool is_write = false;
    std::uint8_t reg = 0;
    std::size_t width = 0;
    bool sign_extend = false;
    std::uintptr_t instruction_length = 0;
};

static std::size_t access_width_size(AccessWidth width) {
    return static_cast<std::size_t>(width);
}

static std::uintptr_t get_reg(const KvmRegs &regs, std::uint8_t index) {
    if (index == 0 || index >= kvm_reg_index(KvmReg::Count)) {
        return 0;
    }
    return regs[index];
}

static void set_reg(KvmRegs *regs, std::uint8_t index, std::uintptr_t value) {
    if (regs == nullptr || index == 0 || index >= kvm_reg_index(KvmReg::Count)) {
        return;
    }
    (*regs)[index] = value;
}

static bool decode_load(std::uintptr_t inst, std::uintptr_t instruction_length, DecodedPageFaultInst *decoded) {
    const auto funct3 = static_cast<LoadFunct3>((inst >> 12) & FUNCT3_MASK);
    decoded->is_write = false;
    decoded->reg = static_cast<std::uint8_t>((inst >> 7) & REG_INDEX_MASK);
    decoded->instruction_length = instruction_length;

    switch (funct3) {
        case LoadFunct3::Lb:
            decoded->width = access_width_size(AccessWidth::Byte);
            decoded->sign_extend = true;
            return true;
        case LoadFunct3::Lh:
            decoded->width = access_width_size(AccessWidth::Halfword);
            decoded->sign_extend = true;
            return true;
        case LoadFunct3::Lw:
            decoded->width = access_width_size(AccessWidth::Word);
            decoded->sign_extend = true;
            return true;
        case LoadFunct3::Ld:
            decoded->width = access_width_size(AccessWidth::Doubleword);
            decoded->sign_extend = true;
            return true;
        case LoadFunct3::Lbu:
            decoded->width = access_width_size(AccessWidth::Byte);
            decoded->sign_extend = false;
            return true;
        case LoadFunct3::Lhu:
            decoded->width = access_width_size(AccessWidth::Halfword);
            decoded->sign_extend = false;
            return true;
        case LoadFunct3::Lwu:
            decoded->width = access_width_size(AccessWidth::Word);
            decoded->sign_extend = false;
            return true;
        default:
            return false;
    }
}

static bool decode_store(std::uintptr_t inst, std::uintptr_t instruction_length, DecodedPageFaultInst *decoded) {
    const auto funct3 = static_cast<StoreFunct3>((inst >> 12) & FUNCT3_MASK);
    decoded->is_write = true;
    decoded->reg = static_cast<std::uint8_t>((inst >> 20) & REG_INDEX_MASK);
    decoded->instruction_length = instruction_length;
    decoded->sign_extend = false;

    switch (funct3) {
        case StoreFunct3::Sb:
            decoded->width = access_width_size(AccessWidth::Byte);
            return true;
        case StoreFunct3::Sh:
            decoded->width = access_width_size(AccessWidth::Halfword);
            return true;
        case StoreFunct3::Sw:
            decoded->width = access_width_size(AccessWidth::Word);
            return true;
        case StoreFunct3::Sd:
            decoded->width = access_width_size(AccessWidth::Doubleword);
            return true;
        default:
            return false;
    }
}

static bool decode_page_fault_inst(const KvmPageFault &page_fault, DecodedPageFaultInst *decoded) {
    const std::uintptr_t raw = page_fault.inst;
    if ((raw & ILLEGAL_COMPRESSED_INST_LOW_BIT) == 0) {
        return false;
    }
    if (sizeof(std::uintptr_t) > INSTRUCTION_32BIT_SIZE && (raw >> 32) != 0) {
        return false;
    }

    const std::uintptr_t inst = raw | 0x2;
    const std::uintptr_t instruction_length =
        (raw & UNCOMPRESSED_INST_LOW_BITS) == UNCOMPRESSED_INST_LOW_BITS ? INSTRUCTION_32BIT_SIZE
                                                                         : INSTRUCTION_16BIT_SIZE;
    switch (static_cast<Opcode>(inst & OPCODE_MASK)) {
        case Opcode::Load:
            return decode_load(inst, instruction_length, decoded);
        case Opcode::Store:
            return decode_store(inst, instruction_length, decoded);
        default:
            return false;
    }
}

static std::uintptr_t maybe_sign_extend(std::uint64_t value, std::size_t width, bool sign_extend) {
    if (!sign_extend) {
        return static_cast<std::uintptr_t>(value);
    }

    const unsigned int bits = static_cast<unsigned int>(width * 8);
    if (bits >= sizeof(std::uintptr_t) * 8) {
        return static_cast<std::uintptr_t>(value);
    }

    const std::uint64_t sign_bit = static_cast<std::uint64_t>(1) << (bits - 1);
    const std::uint64_t mask = ~static_cast<std::uint64_t>(0) << bits;
    if ((value & sign_bit) != 0) {
        value |= mask;
    }

    return static_cast<std::uintptr_t>(value);
}

static std::uint64_t mask_to_width(std::uintptr_t value, std::size_t width) {
    if (width >= sizeof(std::uint64_t)) {
        return static_cast<std::uint64_t>(value);
    }

    return static_cast<std::uint64_t>(value) & ((static_cast<std::uint64_t>(1) << (width * 8)) - 1);
}

static MemoryFaultAccess memory_fault_access(std::uintptr_t access_type) {
    return static_cast<MemoryFaultAccess>(access_type);
}

static const char *memory_access_name(MemoryFaultAccess access_type) {
    switch (access_type) {
        case MemoryFaultAccess::Read:
            return "read";
        case MemoryFaultAccess::Write:
            return "write";
        case MemoryFaultAccess::Execute:
            return "execute";
        default:
            return "unknown";
    }
}

bool KvmCpu::handle_memory_fault() const {
    if (this->bus_ == nullptr) {
        std::fprintf(stderr, "kvm memory fault cannot be handled without bus\n");
        return false;
    }

    KvmPageFault page_fault = {};
    if (!this->get_page_fault(page_fault)) {
        return false;
    }

    const MemoryFaultAccess access_type = memory_fault_access(page_fault.access_type);
    if (access_type == MemoryFaultAccess::Execute) {
        std::fprintf(stderr, "unsupported kvm memory fault: addr=0x%lx access=%s\n",
                     static_cast<unsigned long>(page_fault.addr), memory_access_name(access_type));
        return false;
    }

    DecodedPageFaultInst decoded = {};
    if (!decode_page_fault_inst(page_fault, &decoded) ||
        decoded.is_write != (access_type == MemoryFaultAccess::Write)) {
        std::fprintf(stderr, "unsupported kvm page fault instruction: addr=0x%lx access=0x%lx inst=0x%lx\n",
                     static_cast<unsigned long>(page_fault.addr), static_cast<unsigned long>(page_fault.access_type),
                     static_cast<unsigned long>(page_fault.inst));
        return false;
    }

    KvmRegs regs = {};
    if (!this->get_regs(regs)) {
        return false;
    }
    if (decoded.is_write) {
        const std::uint64_t value = mask_to_width(get_reg(regs, decoded.reg), decoded.width);
        if (!this->bus_->write_mmio(page_fault.addr, decoded.width, value)) {
            std::fprintf(stderr, "unsupported kvm mmio write: addr=0x%lx width=%zu value=0x%lx\n",
                         static_cast<unsigned long>(page_fault.addr), decoded.width, static_cast<unsigned long>(value));
            return false;
        }
    } else if (access_type == MemoryFaultAccess::Read) {
        std::uint64_t value = 0;
        if (!this->bus_->read_mmio(page_fault.addr, decoded.width, &value)) {
            std::fprintf(stderr, "unsupported kvm mmio read: addr=0x%lx width=%zu\n",
                         static_cast<unsigned long>(page_fault.addr), decoded.width);
            return false;
        }
        set_reg(&regs, decoded.reg, maybe_sign_extend(value, decoded.width, decoded.sign_extend));
    } else {
        std::fprintf(stderr, "unsupported kvm memory fault: addr=0x%lx access=%s\n",
                     static_cast<unsigned long>(page_fault.addr), memory_access_name(access_type));
        return false;
    }

    regs[KvmReg::Pc] += decoded.instruction_length;
    return this->set_regs(regs);
}
} // namespace kvm_host
