use std::mem;

use num_enum::TryFromPrimitive;

use crate::abi::{KvmPageFault, KvmReg};
use crate::vcpu::KvmCpu;

pub fn handle_memory_fault(cpu: &KvmCpu) -> Result<(), String> {
    let page_fault = cpu.get_page_fault()?;
    let Ok(access_type) = MemoryFaultAccess::try_from(page_fault.access_type) else {
        return Err(format!(
            "unsupported kvm memory fault: addr=0x{:x} access=0x{:x}",
            page_fault.addr, page_fault.access_type
        ));
    };
    if access_type == MemoryFaultAccess::Execute {
        return Err(format!(
            "unsupported kvm memory fault: addr=0x{:x} access=execute",
            page_fault.addr
        ));
    }
    let Some(decoded) = decode_page_fault_inst(page_fault) else {
        return Err(format!(
            "unsupported kvm page fault instruction: addr=0x{:x} access=0x{:x} inst=0x{:x}",
            page_fault.addr, page_fault.access_type, page_fault.inst
        ));
    };
    if decoded.is_write != (access_type == MemoryFaultAccess::Write) {
        return Err(format!(
            "unsupported kvm page fault instruction: addr=0x{:x} access=0x{:x} inst=0x{:x}",
            page_fault.addr, page_fault.access_type, page_fault.inst
        ));
    }

    let mut regs = cpu.get_regs()?;
    if decoded.is_write {
        let value = mask_to_width(regs.get_index(decoded.reg), decoded.width);
        if !cpu.bus().borrow().write_mmio(page_fault.addr, decoded.width, value) {
            eprintln!(
                "warning: unsupported kvm mmio write: addr=0x{:x} width={} value=0x{:x}",
                page_fault.addr, decoded.width, value
            );
        }
    } else {
        let value = match cpu.bus().borrow().read_mmio(page_fault.addr, decoded.width) {
            Some(value) => value,
            None => {
                eprintln!(
                    "warning: unsupported kvm mmio read: addr=0x{:x} width={}",
                    page_fault.addr, decoded.width
                );
                0
            }
        };
        regs.set_index(
            decoded.reg,
            maybe_sign_extend(value, decoded.width, decoded.sign_extend),
        );
    }

    regs.set(
        KvmReg::Pc,
        regs.get(KvmReg::Pc).wrapping_add(decoded.instruction_length),
    );
    cpu.set_regs(&regs)
}

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
enum MemoryFaultAccess {
    Read = 0,
    Write = 1,
    Execute = 2,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum Opcode {
    Load = 0x03,
    Store = 0x23,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum LoadFunct3 {
    Lb = 0,
    Lh = 1,
    Lw = 2,
    Ld = 3,
    Lbu = 4,
    Lhu = 5,
    Lwu = 6,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum StoreFunct3 {
    Sb = 0,
    Sh = 1,
    Sw = 2,
    Sd = 3,
}

struct DecodedPageFaultInst {
    is_write: bool,
    reg: u8,
    width: usize,
    sign_extend: bool,
    instruction_length: usize,
}

fn decode_page_fault_inst(page_fault: KvmPageFault) -> Option<DecodedPageFaultInst> {
    let raw = page_fault.inst;
    if (raw & 0x1) == 0 || (usize::BITS > 32 && (raw >> 32) != 0) {
        return None;
    }
    let inst = raw | 0x2;
    let instruction_length = if (raw & 0x3) == 0x3 { 4 } else { 2 };
    let opcode = Opcode::try_from(inst & 0x7f).ok()?;
    let funct3 = (inst >> 12) & 0x7;
    match opcode {
        Opcode::Load => {
            let (width, sign_extend) = match LoadFunct3::try_from(funct3).ok()? {
                LoadFunct3::Lb => (1, true),
                LoadFunct3::Lh => (2, true),
                LoadFunct3::Lw => (4, true),
                LoadFunct3::Ld => (8, true),
                LoadFunct3::Lbu => (1, false),
                LoadFunct3::Lhu => (2, false),
                LoadFunct3::Lwu => (4, false),
            };
            Some(DecodedPageFaultInst {
                is_write: false,
                reg: ((inst >> 7) & 0x1f) as u8,
                width,
                sign_extend,
                instruction_length,
            })
        }
        Opcode::Store => {
            let width = match StoreFunct3::try_from(funct3).ok()? {
                StoreFunct3::Sb => 1,
                StoreFunct3::Sh => 2,
                StoreFunct3::Sw => 4,
                StoreFunct3::Sd => 8,
            };
            Some(DecodedPageFaultInst {
                is_write: true,
                reg: ((inst >> 20) & 0x1f) as u8,
                width,
                sign_extend: false,
                instruction_length,
            })
        }
    }
}

fn maybe_sign_extend(mut value: u64, width: usize, sign_extend: bool) -> usize {
    if !sign_extend {
        return value as usize;
    }
    let bits = width * 8;
    if bits >= usize::BITS as usize {
        return value as usize;
    }
    let sign_bit = 1u64 << (bits - 1);
    let mask = u64::MAX << bits;
    if (value & sign_bit) != 0 {
        value |= mask;
    }
    value as usize
}

fn mask_to_width(value: usize, width: usize) -> u64 {
    if width >= mem::size_of::<u64>() {
        value as u64
    } else {
        (value as u64) & ((1u64 << (width * 8)) - 1)
    }
}
