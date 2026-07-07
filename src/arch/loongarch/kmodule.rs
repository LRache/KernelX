use core::mem::size_of;

use num_enum::TryFromPrimitive;

use crate::kernel::errno::{Errno, SysResult};
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
enum LoongArchRelocation {
    None = 0,
    Abs64 = 2,
    B26 = 66,
    PcalaHi20 = 71,
    PcalaLo12 = 72,
    GotPcHi20 = 75,
    GotPcLo12 = 76,
    Call36 = 110,
}

pub fn relocation_action(relocation_type: u32) -> SysResult<KModuleRelocationAction> {
    let relocation = LoongArchRelocation::try_from(relocation_type).map_err(|_| Errno::ENOEXEC)?;
    Ok(match relocation {
        LoongArchRelocation::None => KModuleRelocationAction::None,
        LoongArchRelocation::Abs64
        | LoongArchRelocation::B26
        | LoongArchRelocation::PcalaHi20
        | LoongArchRelocation::PcalaLo12
        | LoongArchRelocation::Call36 => KModuleRelocationAction::ResolveSymbol,
        LoongArchRelocation::GotPcHi20 | LoongArchRelocation::GotPcLo12 => KModuleRelocationAction::ResolveGotEntry,
    })
}

pub fn apply_relocation(
    relocation_type: u32,
    place: &mut [u8],
    value: Option<KModuleRelocationValue>,
) -> SysResult<()> {
    let relocation = LoongArchRelocation::try_from(relocation_type).map_err(|_| Errno::ENOEXEC)?;
    match relocation {
        LoongArchRelocation::None => Ok(()),
        LoongArchRelocation::Abs64 => write_abs64(place, resolved_relocation(value)?.1),
        LoongArchRelocation::B26 => {
            let (base, value) = resolved_relocation(value)?;
            write_branch26(place, pcrel_offset(value, base)?)
        }
        LoongArchRelocation::Call36 => {
            let (base, value) = resolved_relocation(value)?;
            write_call36(place, pcrel_offset(value, base)?)
        }
        LoongArchRelocation::PcalaHi20 => {
            let (base, value) = resolved_relocation(value)?;
            write_pcala_hi20(place, base, value)
        }
        LoongArchRelocation::GotPcHi20 => {
            let (base, value) = resolved_relocation(value)?;
            write_pcala_hi20(place, base, value)
        }
        LoongArchRelocation::PcalaLo12 => write_page_offset12(place, resolved_relocation(value)?.1),
        LoongArchRelocation::GotPcLo12 => write_page_offset12(place, resolved_relocation(value)?.1),
    }
}

fn resolved_relocation(value: Option<KModuleRelocationValue>) -> SysResult<(usize, usize)> {
    let value = value.ok_or(Errno::ENOEXEC)?;
    Ok((value.base, value.value))
}

pub fn flush_icache() {
    // SAFETY: dbar/ibar are local ordering barriers with no operands; after
    // patching module text, they make the written instructions visible before
    // execution continues through that text.
    unsafe {
        core::arch::asm!("dbar 0", "ibar 0", options(nostack, preserves_flags));
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

fn write_branch26(place: &mut [u8], offset: i64) -> SysResult<()> {
    if offset % 4 != 0 || !is_signed(offset as i128, 28) {
        return Err(Errno::ENOEXEC);
    }

    let imm = offset >> 2;
    if !is_signed(imm as i128, 26) {
        return Err(Errno::ENOEXEC);
    }

    let imm = imm as u32;
    let imm15_0 = (imm & 0xffff) << 10;
    let imm25_16 = (imm >> 16) & 0x3ff;
    write_instruction(place, |instruction| (instruction & !0x03ff_ffff) | imm15_0 | imm25_16)
}

fn write_call36(place: &mut [u8], offset: i64) -> SysResult<()> {
    if place.len() < size_of::<u32>() * 2 {
        return Err(Errno::ENOEXEC);
    }
    if offset % 4 != 0 || !is_signed((offset as i128) + 0x20000, 38) {
        return Err(Errno::ENOEXEC);
    }

    let imm = offset >> 2;
    if !is_signed(imm as i128, 36) {
        return Err(Errno::ENOEXEC);
    }

    let hi20 = extract_bits((offset + (1 << 17)) as u64, 37, 18) << 5;
    let lo16 = extract_bits(offset as u64, 17, 2) << 10;
    write_instruction(place, |instruction| (instruction & !(0xfffff << 5)) | hi20)?;
    write_instruction(&mut place[size_of::<u32>()..], |instruction| {
        (instruction & !(0xffff << 10)) | lo16
    })
}

fn write_pcala_hi20(place: &mut [u8], base: usize, value: usize) -> SysResult<()> {
    let target = value.checked_add(value & 0x800).ok_or(Errno::ENOEXEC)?;
    let target_page = target & !0xfff;
    let base_page = base & !0xfff;
    let page_delta = target_page as i128 - base_page as i128;
    if !is_signed(page_delta, 32) {
        return Err(Errno::ENOEXEC);
    }

    let imm = ((page_delta >> 12) as u32 & 0xfffff) << 5;
    write_instruction(place, |instruction| (instruction & !(0xfffff << 5)) | imm)
}

fn write_page_offset12(place: &mut [u8], value: usize) -> SysResult<()> {
    let imm = ((value & 0xfff) as u32) << 10;
    write_instruction(place, |instruction| (instruction & !(0xfff << 10)) | imm)
}

fn write_instruction(place: &mut [u8], write: impl FnOnce(u32) -> u32) -> SysResult<()> {
    if place.len() < size_of::<u32>() {
        return Err(Errno::ENOEXEC);
    }

    let mut bytes = [0; size_of::<u32>()];
    bytes.copy_from_slice(&place[..size_of::<u32>()]);
    let instruction = write(u32::from_le_bytes(bytes));
    place[..size_of::<u32>()].copy_from_slice(&instruction.to_le_bytes());
    Ok(())
}

fn is_signed(value: i128, bits: u32) -> bool {
    let min = -(1i128 << (bits - 1));
    let max = (1i128 << (bits - 1)) - 1;
    min <= value && value <= max
}

fn extract_bits(value: u64, hi: u32, lo: u32) -> u32 {
    ((value >> lo) & ((1 << (hi - lo + 1)) - 1)) as u32
}
