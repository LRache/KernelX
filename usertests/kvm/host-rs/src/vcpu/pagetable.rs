use std::{mem, ptr};

use num_enum::TryFromPrimitive;

use crate::device::bus::{Bus, checked_range_end};
use crate::vcpu::KvmCpu;

const PAGE_SHIFT: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

impl KvmCpu {
    pub fn translate_read(&self, guest_vaddr: usize, buffer: &mut [u8]) -> bool {
        let bus = self.bus();
        let bus = bus.lock().expect("kvm bus lock poisoned");
        let Some(chunks) = self.translate_chunks(&bus, guest_vaddr, buffer.len(), TranslateAccess::Read) else {
            return false;
        };

        for (host_addr, offset, chunk) in chunks {
            unsafe {
                ptr::copy(host_addr, buffer.as_mut_ptr().add(offset), chunk);
            }
        }
        true
    }

    #[allow(dead_code)]
    pub fn translate_write(&self, guest_vaddr: usize, buffer: &[u8]) -> bool {
        let bus = self.bus();
        let bus = bus.lock().expect("kvm bus lock poisoned");
        let Some(chunks) = self.translate_chunks(&bus, guest_vaddr, buffer.len(), TranslateAccess::Write) else {
            return false;
        };

        for (host_addr, offset, chunk) in chunks {
            unsafe {
                ptr::copy(buffer.as_ptr().add(offset), host_addr, chunk);
            }
        }
        true
    }

    fn translate_chunks(
        &self,
        bus: &Bus,
        mut guest_vaddr: usize,
        length: usize,
        access: TranslateAccess,
    ) -> Option<Vec<(*mut u8, usize, usize)>> {
        if length == 0 {
            return Some(Vec::new());
        }
        checked_range_end(guest_vaddr, length)?;
        let satp = self.get_sregs().ok()?.satp;

        let mut offset = 0;
        let mut chunks = Vec::new();
        while offset < length {
            let chunk = guest_page_chunk(guest_vaddr, length - offset);
            let host_addr = self.translate_guest_vaddr(bus, satp, guest_vaddr, chunk, access)?;
            chunks.push((host_addr, offset, chunk));
            guest_vaddr += chunk;
            offset += chunk;
        }
        Some(chunks)
    }

    fn translate_guest_vaddr(
        &self,
        bus: &Bus,
        satp: usize,
        guest_vaddr: usize,
        length: usize,
        access: TranslateAccess,
    ) -> Option<*mut u8> {
        let end = checked_range_end(guest_vaddr, length)?;
        match SatpMode::try_from(satp >> 60).ok()? {
            SatpMode::Bare => return bus.translate(guest_vaddr, length),
            SatpMode::Sv39 => {}
        }
        if !is_sv39_canonical(guest_vaddr) || !is_sv39_canonical(end - 1) {
            return None;
        }

        let mut table_paddr = (satp & ((1usize << 44) - 1)) << 12;
        for level in (0..=2).rev() {
            let pte_addr = table_paddr + vpn_index(guest_vaddr, level) * 8;
            let pte = read_pte(bus, pte_addr)?;
            if !is_valid_pte(pte) {
                return None;
            }
            let ppn = ((pte >> 10) & ((1u64 << 44) - 1)) as usize;
            if is_leaf_pte(pte) {
                if !access.allowed_by(pte) {
                    return None;
                }
                let lower_ppn_mask = (1usize << (level * 9)) - 1;
                if (ppn & lower_ppn_mask) != 0 || !range_fits_in_leaf(guest_vaddr, length, level) {
                    return None;
                }
                let page_shift = 12 + level * 9;
                let page_offset = guest_vaddr & ((1usize << page_shift) - 1);
                let guest_paddr = (ppn << 12) | page_offset;
                return bus.translate(guest_paddr, length);
            }
            table_paddr = ppn << 12;
        }
        None
    }
}

#[derive(Clone, Copy)]
enum TranslateAccess {
    Read,
    Write,
}

impl TranslateAccess {
    fn allowed_by(self, pte: u64) -> bool {
        match self {
            Self::Read => PteFlag::Read.is_set(pte),
            Self::Write => PteFlag::Write.is_set(pte),
        }
    }
}

#[repr(u64)]
#[derive(Clone, Copy)]
enum PteFlag {
    Valid = 1 << 0,
    Read = 1 << 1,
    Write = 1 << 2,
    Execute = 1 << 3,
}

impl PteFlag {
    fn is_set(self, pte: u64) -> bool {
        (pte & self as u64) != 0
    }
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SatpMode {
    Bare = 0,
    Sv39 = 8,
}

fn guest_page_chunk(guest_vaddr: usize, remaining: usize) -> usize {
    let page_remaining = PAGE_SIZE - (guest_vaddr & (PAGE_SIZE - 1));
    remaining.min(page_remaining)
}

fn is_sv39_canonical(vaddr: usize) -> bool {
    let sign_bit = 1usize << 38;
    let lower_mask = (1usize << 39) - 1;
    let upper = vaddr & !lower_mask;
    if (vaddr & sign_bit) == 0 {
        upper == 0
    } else {
        upper == !lower_mask
    }
}

fn vpn_index(vaddr: usize, level: usize) -> usize {
    (vaddr >> (PAGE_SHIFT + level * 9)) & ((1 << 9) - 1)
}

fn read_pte(bus: &Bus, paddr: usize) -> Option<u64> {
    let host_addr = bus.translate(paddr, mem::size_of::<u64>())?;
    let mut value = 0u64;
    unsafe {
        ptr::copy_nonoverlapping(host_addr, &mut value as *mut u64 as *mut u8, mem::size_of::<u64>());
    }
    Some(value)
}

fn is_valid_pte(pte: u64) -> bool {
    let valid = PteFlag::Valid.is_set(pte);
    let readable = PteFlag::Read.is_set(pte);
    let writable = PteFlag::Write.is_set(pte);
    valid && (readable || !writable)
}

fn is_leaf_pte(pte: u64) -> bool {
    PteFlag::Read.is_set(pte) || PteFlag::Execute.is_set(pte)
}

fn range_fits_in_leaf(vaddr: usize, length: usize, level: usize) -> bool {
    let page_size = 1usize << (PAGE_SHIFT + level * 9);
    let page_offset = vaddr & (page_size - 1);
    length <= page_size - page_offset
}
