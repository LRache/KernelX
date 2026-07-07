use core::mem::size_of;
use num_enum::TryFromPrimitive;

use crate::kernel::errno::{Errno, SysResult};
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
enum RiscvRelocation {
    None = 0,
    Abs64 = 2,
    CallPlt = 19,
    GotHi20 = 20,
    PcrelHi20 = 23,
    PcrelLo12I = 24,
}

pub fn relocation_action(relocation_type: u32) -> SysResult<KModuleRelocationAction> {
    let relocation = RiscvRelocation::try_from(relocation_type).map_err(|_| Errno::ENOEXEC)?;
    Ok(match relocation {
        RiscvRelocation::None => KModuleRelocationAction::None,
        RiscvRelocation::Abs64 => KModuleRelocationAction::ResolveSymbol,
        RiscvRelocation::CallPlt => KModuleRelocationAction::ResolveSymbol,
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
        RiscvRelocation::GotHi20 | RiscvRelocation::PcrelHi20 => {
            let (base, value) = resolved_relocation(value)?;
            write_u_type(place, pcrel_offset(value, base)?)
        }
        RiscvRelocation::PcrelLo12I => {
            let (base, value) = resolved_relocation(value)?;
            write_i_type(place, pcrel_offset(value, base)?)
        }
        RiscvRelocation::CallPlt => {
            let (base, value) = resolved_relocation(value)?;
            if place.len() < size_of::<u32>() * 2 {
                return Err(Errno::ENOEXEC);
            }
            let offset = pcrel_offset(value, base)?;
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
    if place.len() < size_of::<u32>() {
        return Err(Errno::ENOEXEC);
    }
    let hi20 = ((offset + 0x800) >> 12) as u32;
    let mut bytes = [0; size_of::<u32>()];
    bytes.copy_from_slice(&place[..size_of::<u32>()]);
    let instruction = u32::from_le_bytes(bytes);
    let instruction = (instruction & 0xfff) | (hi20 << 12);
    place[..size_of::<u32>()].copy_from_slice(&instruction.to_le_bytes());
    Ok(())
}

fn write_i_type(place: &mut [u8], offset: i64) -> SysResult<()> {
    if place.len() < size_of::<u32>() {
        return Err(Errno::ENOEXEC);
    }
    let hi20 = (offset + 0x800) >> 12;
    let lo12 = offset - (hi20 << 12);
    let mut bytes = [0; size_of::<u32>()];
    bytes.copy_from_slice(&place[..size_of::<u32>()]);
    let instruction = u32::from_le_bytes(bytes);
    let instruction = (instruction & !(0xfff << 20)) | (((lo12 as u32) & 0xfff) << 20);
    place[..size_of::<u32>()].copy_from_slice(&instruction.to_le_bytes());
    Ok(())
}
