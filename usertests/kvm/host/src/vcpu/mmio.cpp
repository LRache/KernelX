#include "vcpu.hpp"

#include "bus.hpp"

#include <cstddef>
#include <cstdint>
#include <cstdio>

namespace kvm_host {
static constexpr std::uintptr_t MMIO_ACCESS_READ = 0;
static constexpr std::uintptr_t MMIO_ACCESS_WRITE = 1;

static constexpr std::uintptr_t OPCODE_LOAD = 0x03;
static constexpr std::uintptr_t OPCODE_STORE = 0x23;
static constexpr std::uintptr_t OPCODE_MASK = 0x7f;

struct DecodedPageFaultInst {
    bool is_write = false;
    std::uint8_t reg = 0;
    std::size_t width = 0;
    bool sign_extend = false;
    std::uintptr_t instruction_length = 0;
};

static std::uintptr_t get_reg(const KvmRegs &regs, std::uint8_t index) {
    switch (index) {
        case 0:
            return 0;
        case 1:
            return regs.ra;
        case 2:
            return regs.sp;
        case 3:
            return regs.gp;
        case 4:
            return regs.tp;
        case 5:
            return regs.t0;
        case 6:
            return regs.t1;
        case 7:
            return regs.t2;
        case 8:
            return regs.s0;
        case 9:
            return regs.s1;
        case 10:
            return regs.a0;
        case 11:
            return regs.a1;
        case 12:
            return regs.a2;
        case 13:
            return regs.a3;
        case 14:
            return regs.a4;
        case 15:
            return regs.a5;
        case 16:
            return regs.a6;
        case 17:
            return regs.a7;
        case 18:
            return regs.s2;
        case 19:
            return regs.s3;
        case 20:
            return regs.s4;
        case 21:
            return regs.s5;
        case 22:
            return regs.s6;
        case 23:
            return regs.s7;
        case 24:
            return regs.s8;
        case 25:
            return regs.s9;
        case 26:
            return regs.s10;
        case 27:
            return regs.s11;
        case 28:
            return regs.t3;
        case 29:
            return regs.t4;
        case 30:
            return regs.t5;
        case 31:
            return regs.t6;
        default:
            return 0;
    }
}

static void set_reg(KvmRegs *regs, std::uint8_t index, std::uintptr_t value) {
    switch (index) {
        case 0:
            return;
        case 1:
            regs->ra = value;
            return;
        case 2:
            regs->sp = value;
            return;
        case 3:
            regs->gp = value;
            return;
        case 4:
            regs->tp = value;
            return;
        case 5:
            regs->t0 = value;
            return;
        case 6:
            regs->t1 = value;
            return;
        case 7:
            regs->t2 = value;
            return;
        case 8:
            regs->s0 = value;
            return;
        case 9:
            regs->s1 = value;
            return;
        case 10:
            regs->a0 = value;
            return;
        case 11:
            regs->a1 = value;
            return;
        case 12:
            regs->a2 = value;
            return;
        case 13:
            regs->a3 = value;
            return;
        case 14:
            regs->a4 = value;
            return;
        case 15:
            regs->a5 = value;
            return;
        case 16:
            regs->a6 = value;
            return;
        case 17:
            regs->a7 = value;
            return;
        case 18:
            regs->s2 = value;
            return;
        case 19:
            regs->s3 = value;
            return;
        case 20:
            regs->s4 = value;
            return;
        case 21:
            regs->s5 = value;
            return;
        case 22:
            regs->s6 = value;
            return;
        case 23:
            regs->s7 = value;
            return;
        case 24:
            regs->s8 = value;
            return;
        case 25:
            regs->s9 = value;
            return;
        case 26:
            regs->s10 = value;
            return;
        case 27:
            regs->s11 = value;
            return;
        case 28:
            regs->t3 = value;
            return;
        case 29:
            regs->t4 = value;
            return;
        case 30:
            regs->t5 = value;
            return;
        case 31:
            regs->t6 = value;
            return;
        default:
            return;
    }
}

static bool decode_load(std::uintptr_t inst, std::uintptr_t instruction_length, DecodedPageFaultInst *decoded) {
    const std::uintptr_t funct3 = (inst >> 12) & 0x7;
    decoded->is_write = false;
    decoded->reg = static_cast<std::uint8_t>((inst >> 7) & 0x1f);
    decoded->instruction_length = instruction_length;

    switch (funct3) {
        case 0:
            decoded->width = 1;
            decoded->sign_extend = true;
            return true;
        case 1:
            decoded->width = 2;
            decoded->sign_extend = true;
            return true;
        case 2:
            decoded->width = 4;
            decoded->sign_extend = true;
            return true;
        case 3:
            decoded->width = 8;
            decoded->sign_extend = true;
            return true;
        case 4:
            decoded->width = 1;
            decoded->sign_extend = false;
            return true;
        case 5:
            decoded->width = 2;
            decoded->sign_extend = false;
            return true;
        case 6:
            decoded->width = 4;
            decoded->sign_extend = false;
            return true;
        default:
            return false;
    }
}

static bool decode_store(std::uintptr_t inst, std::uintptr_t instruction_length, DecodedPageFaultInst *decoded) {
    const std::uintptr_t funct3 = (inst >> 12) & 0x7;
    decoded->is_write = true;
    decoded->reg = static_cast<std::uint8_t>((inst >> 20) & 0x1f);
    decoded->instruction_length = instruction_length;
    decoded->sign_extend = false;

    switch (funct3) {
        case 0:
            decoded->width = 1;
            return true;
        case 1:
            decoded->width = 2;
            return true;
        case 2:
            decoded->width = 4;
            return true;
        case 3:
            decoded->width = 8;
            return true;
        default:
            return false;
    }
}

static bool decode_page_fault_inst(const KvmPageFault &page_fault, DecodedPageFaultInst *decoded) {
    const std::uintptr_t raw = page_fault.inst;
    if ((raw & 0x1) == 0) {
        return false;
    }
    if (sizeof(std::uintptr_t) > 4 && (raw >> 32) != 0) {
        return false;
    }

    const std::uintptr_t inst = raw | 0x2;
    const std::uintptr_t instruction_length = (raw & 0x2) == 0 ? 2 : 4;
    switch (inst & OPCODE_MASK) {
        case OPCODE_LOAD:
            return decode_load(inst, instruction_length, decoded);
        case OPCODE_STORE:
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

bool KvmCpu::handle_mmio_fault(std::uintptr_t fault_addr, std::uintptr_t access_type) const {
    if (bus_ == nullptr) {
        return false;
    }

    KvmPageFault page_fault = {};
    if (!get_page_fault(page_fault)) {
        return false;
    }
    if (page_fault.addr != fault_addr || page_fault.access_type != access_type) {
        std::fprintf(stderr,
                     "stale kvm page fault info: fault addr=0x%lx access=0x%lx page fault addr=0x%lx access=0x%lx\n",
                     static_cast<unsigned long>(fault_addr), static_cast<unsigned long>(access_type),
                     static_cast<unsigned long>(page_fault.addr), static_cast<unsigned long>(page_fault.access_type));
        return false;
    }

    DecodedPageFaultInst decoded = {};
    if (!decode_page_fault_inst(page_fault, &decoded) ||
        decoded.is_write != (page_fault.access_type == MMIO_ACCESS_WRITE)) {
        std::fprintf(stderr, "unsupported kvm page fault instruction: addr=0x%lx access=0x%lx inst=0x%lx\n",
                     static_cast<unsigned long>(page_fault.addr), static_cast<unsigned long>(page_fault.access_type),
                     static_cast<unsigned long>(page_fault.inst));
        return false;
    }

    KvmRegs regs = {};
    if (!get_regs(regs)) {
        return false;
    }
    if (decoded.is_write) {
        const std::uint64_t value = mask_to_width(get_reg(regs, decoded.reg), decoded.width);
        if (!bus_->write_mmio(page_fault.addr, decoded.width, value)) {
            return false;
        }
    } else if (page_fault.access_type == MMIO_ACCESS_READ) {
        std::uint64_t value = 0;
        if (!bus_->read_mmio(page_fault.addr, decoded.width, &value)) {
            return false;
        }
        set_reg(&regs, decoded.reg, maybe_sign_extend(value, decoded.width, decoded.sign_extend));
    } else {
        return false;
    }

    regs.pc += decoded.instruction_length;
    return set_regs(regs);
}
} // namespace kvm_host
