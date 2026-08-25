use core::mem::size_of;
use num_enum::TryFromPrimitive;

use crate::kernel::errno::{Errno, SysResult};
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
enum RiscvRelocation {
    None = 0,
    Abs64 = 2,
    Branch = 16,
    Jal = 17,
    CallPlt = 19,
    GotHi20 = 20,
    PcrelHi20 = 23,
    PcrelLo12I = 24,
    RvcBranch = 44,
    RvcJump = 45,
}

pub fn relocation_action(relocation_type: u32) -> SysResult<KModuleRelocationAction> {
    let relocation = RiscvRelocation::try_from(relocation_type).map_err(|_| Errno::ENOEXEC)?;
    Ok(match relocation {
        RiscvRelocation::None => KModuleRelocationAction::None,
        RiscvRelocation::Abs64
        | RiscvRelocation::Branch
        | RiscvRelocation::Jal
        | RiscvRelocation::RvcBranch
        | RiscvRelocation::RvcJump
        | RiscvRelocation::CallPlt => KModuleRelocationAction::ResolveSymbol,
        RiscvRelocation::GotHi20 => KModuleRelocationAction::ResolveGotEntry,
        RiscvRelocation::PcrelHi20 => KModuleRelocationAction::ResolveSymbolAndRecordReferenceTarget,
        RiscvRelocation::PcrelLo12I => KModuleRelocationAction::ResolveReferencedRelocation,
    })
}

pub fn apply_relocation(
    relocation_type: u32,
    place: &mut [u8],
    value: Option<KModuleRelocationValue>,
) -> SysResult<()> {
    let relocation = RiscvRelocation::try_from(relocation_type).map_err(|_| Errno::ENOEXEC)?;
    match relocation {
        RiscvRelocation::None => Ok(()),
        RiscvRelocation::Abs64 => write_abs64(place, resolved_relocation(value)?.1),
        RiscvRelocation::Branch => write_b_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::Jal => write_j_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::RvcBranch => write_cb_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::RvcJump => write_cj_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::GotHi20 | RiscvRelocation::PcrelHi20 => write_u_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::PcrelLo12I => write_i_type(place, pcrel_relocation_offset(value)?),
        RiscvRelocation::CallPlt => {
            let offset = pcrel_relocation_offset(value)?;
            if place.len() < size_of::<u32>() * 2 {
                return Err(Errno::ENOEXEC);
            }
            let (hi_place, lo_place) = place.split_at_mut(size_of::<u32>());
            write_u_type(hi_place, offset)?;
            write_i_type(lo_place, offset)?;
            Ok(())
        }
    }
}

fn resolved_relocation(value: Option<KModuleRelocationValue>) -> SysResult<(usize, usize)> {
    let value = value.ok_or(Errno::ENOEXEC)?;
    Ok((value.base, value.value))
}

fn pcrel_relocation_offset(value: Option<KModuleRelocationValue>) -> SysResult<i64> {
    let (base, value) = resolved_relocation(value)?;
    pcrel_offset(value, base)
}

pub fn flush_icache() {
    // SAFETY: fence.i has no operands, does not touch memory directly, and is
    // required after patching module instructions.
    unsafe {
        core::arch::asm!("fence.i", options(nostack, preserves_flags));
    }
}

fn pcrel_offset(value: usize, place: usize) -> SysResult<i64> {
    let offset = value as i128 - place as i128;
    if offset < i64::MIN as i128 || offset > i64::MAX as i128 {
        return Err(Errno::ENOEXEC);
    }
    Ok(offset as i64)
}

fn write_abs64(place: &mut [u8], value: usize) -> SysResult<()> {
    if place.len() < size_of::<u64>() {
        return Err(Errno::ENOEXEC);
    }
    place[..size_of::<u64>()].copy_from_slice(&(value as u64).to_le_bytes());
    Ok(())
}

fn write_u_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    let hi20 = ((offset + 0x800) >> 12) as u32;
    patch_u32_instruction(place, 0xfff, hi20 << 12)
}

fn write_i_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    let hi20 = (offset + 0x800) >> 12;
    let lo12 = offset - (hi20 << 12);
    patch_u32_instruction(place, !(0xfff << 20), ((lo12 as u32) & 0xfff) << 20)
}

// B-type immediate of beq/bne/blt/bge: imm[12|10:5] at bits 31:25 and
// imm[4:1|11] at bits 11:7, keeping opcode/funct3/rs1/rs2 intact.
fn write_b_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    const RANGE: i64 = 1 << 12;
    if !(-RANGE..RANGE).contains(&offset) || offset & 1 != 0 {
        return Err(Errno::ENOEXEC);
    }
    let imm = offset as u32;
    let bits = ((imm & 0x1000) << 19) | ((imm & 0x7e0) << 20) | ((imm & 0x1e) << 7) | ((imm & 0x800) >> 4);
    patch_u32_instruction(place, 0x01ff_f07f, bits)
}

// J-type immediate of jal: imm[20|10:1|11|19:12] at bits 31:12, keeping
// rd/opcode intact.
fn write_j_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    const RANGE: i64 = 1 << 20;
    if !(-RANGE..RANGE).contains(&offset) || offset & 1 != 0 {
        return Err(Errno::ENOEXEC);
    }
    let imm = offset as u32;
    let bits = ((imm & 0x10_0000) << 11) | ((imm & 0x7fe) << 20) | ((imm & 0x800) << 9) | (imm & 0xff000);
    patch_u32_instruction(place, 0xfff, bits)
}

// CB-type immediate of c.beqz/c.bnez: imm[8|4:3] at bits 12:10 and
// imm[7:6|2:1|5] at bits 6:2, keeping funct3/rs1'/op intact.
fn write_cb_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    const RANGE: i64 = 1 << 8;
    if !(-RANGE..RANGE).contains(&offset) || offset & 1 != 0 {
        return Err(Errno::ENOEXEC);
    }
    let imm = offset as u16;
    let bits =
        ((imm & 0x100) << 4) | ((imm & 0x18) << 7) | ((imm & 0xc0) >> 1) | ((imm & 0x6) << 2) | ((imm & 0x20) >> 3);
    patch_u16_instruction(place, 0xe383, bits)
}

// CJ-type immediate of c.j: imm[11|4|9:8|10|6|7|3:1|5] at bits 12:2, keeping
// funct3/op intact.
fn write_cj_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    const RANGE: i64 = 1 << 11;
    if !(-RANGE..RANGE).contains(&offset) || offset & 1 != 0 {
        return Err(Errno::ENOEXEC);
    }
    let imm = offset as u16;
    let bits = ((imm & 0x800) << 1)
        | ((imm & 0x10) << 7)
        | ((imm & 0x300) << 1)
        | ((imm & 0x400) >> 2)
        | ((imm & 0x40) << 1)
        | ((imm & 0x80) >> 1)
        | ((imm & 0xe) << 2)
        | ((imm & 0x20) >> 3);
    patch_u16_instruction(place, 0xe003, bits)
}

fn patch_u32_instruction(place: &mut [u8], mask: u32, bits: u32) -> SysResult<()> {
    if place.len() < size_of::<u32>() {
        return Err(Errno::ENOEXEC);
    }
    let mut bytes = [0; size_of::<u32>()];
    bytes.copy_from_slice(&place[..size_of::<u32>()]);
    let instruction = u32::from_le_bytes(bytes);
    let instruction = (instruction & mask) | bits;
    place[..size_of::<u32>()].copy_from_slice(&instruction.to_le_bytes());
    Ok(())
}

fn patch_u16_instruction(place: &mut [u8], mask: u16, bits: u16) -> SysResult<()> {
    if place.len() < size_of::<u16>() {
        return Err(Errno::ENOEXEC);
    }
    let mut bytes = [0; size_of::<u16>()];
    bytes.copy_from_slice(&place[..size_of::<u16>()]);
    let instruction = u16::from_le_bytes(bytes);
    let instruction = (instruction & mask) | bits;
    place[..size_of::<u16>()].copy_from_slice(&instruction.to_le_bytes());
    Ok(())
}
