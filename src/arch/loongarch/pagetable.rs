use bitflags::bitflags;
use core::fmt;
use core::ptr::NonNull;

use crate::arch::{PageTableTrait, kaddr_to_paddr, paddr_to_kaddr};
use crate::kernel::mm::{self, MapPerm};

use super::{PGBITS, PGMASK, PGSIZE};

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;
const ENTRIES_PER_TABLE: usize = PGSIZE / core::mem::size_of::<u64>(); // 512

bitflags! {
    /// PTE bit layout (PALEN=48, 4 KiB pages, RPLV=0). Matches hardware
    /// directly — no separate mirror. Vol.1 §8.5.1.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PTEFlags: u64 {
        /// Valid. On leaf entries only — intermediate entries store a bare PA.
        const V    = 1 << 0;
        /// Dirty. Set by hardware on first store.
        const D    = 1 << 1;
        /// PLV = 0 (kernel-only).
        const PLV0 = 0 << 2;
        /// PLV = 3 (user-accessible).
        const PLV3 = 3 << 2;
        /// MAT=0, strongly uncached. MMIO via DMW0, so unused in TLB.
        const SUC  = 0 << 4;
        /// MAT=1, cached coherent. Normal RAM pages.
        const CC   = 1 << 4;
        /// MAT=2, weakly uncached.
        const WUC  = 2 << 4;
        /// Global — translation shared across ASIDs.
        const G    = 1 << 6;
        /// Software Accessed bit (LoongArch has no hardware A).
        /// Lives in the reserved bits 7-11 range. Cleared by
        /// `take_access_dirty_bit`.
        const P    = 1 << 7;
        /// Software writable tracking. Source of truth for "will a store
        /// succeed" from the kernel's view.
        const W    = 1 << 8;
        /// Not readable: load raises PIL.
        const NR   = 1 << 61;
        /// Not executable: ifetch raises PIF.
        const NX   = 1 << 62;
        /// Restrict PLV to the exact value in the PTE.
        const RPLV = 1 << 63;
    }
}

impl From<MapPerm> for PTEFlags {
    fn from(perm: MapPerm) -> Self {
        // Start from "no access"; OR in allowed directions. R/X are negative
        // on LoongArch (NR/NX), so absence in MapPerm sets them.
        let mut flags = PTEFlags::V | PTEFlags::P | PTEFlags::CC;

        if perm.contains(MapPerm::W) {
            flags |= PTEFlags::W;
        }
        if !perm.contains(MapPerm::R) {
            flags |= PTEFlags::NR;
        }
        if !perm.contains(MapPerm::X) {
            flags |= PTEFlags::NX;
        }
        if perm.contains(MapPerm::U) {
            flags |= PTEFlags::PLV3;
        }
        flags
    }
}

impl From<PTEFlags> for MapPerm {
    fn from(flags: PTEFlags) -> Self {
        let mut perm = MapPerm::empty();
        if !flags.contains(PTEFlags::NR) {
            perm |= MapPerm::R;
        }
        if flags.contains(PTEFlags::W) {
            perm |= MapPerm::W;
        }
        if !flags.contains(PTEFlags::NX) {
            perm |= MapPerm::X;
        }
        if flags.contains(PTEFlags::PLV3) {
            perm |= MapPerm::U;
        }
        perm
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr(usize);

impl Addr {
    pub const fn from_vaddr(vaddr: usize) -> Self {
        Self(vaddr)
    }
    pub const fn from_kaddr(kaddr: usize) -> Self {
        Self(kaddr)
    }
    pub fn from_paddr(paddr: usize) -> Self {
        Self(paddr_to_kaddr(paddr))
    }

    pub const fn kaddr(self) -> usize {
        self.0
    }
    pub fn paddr(self) -> usize {
        kaddr_to_paddr(self.0)
    }

    /// 48-bit VA split into three 9-bit indices (top to bottom).
    pub const fn vpn(self) -> [usize; PAGE_TABLE_LEVELS] {
        [(self.0 >> 30) & 0x1ff, (self.0 >> 21) & 0x1ff, (self.0 >> 12) & 0x1ff]
    }

    /// PPN for embedding in a PTE. PA is taken first so DMW1 bits don't leak.
    pub fn ppn(self) -> PPN {
        PPN::from_paddr(self.paddr())
    }

    pub const fn ptr(self) -> *mut u64 {
        self.0 as *mut u64
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PPN(usize);

impl PPN {
    pub const fn from_paddr(paddr: usize) -> Self {
        Self(paddr >> PGBITS)
    }
    pub const fn value(self) -> usize {
        self.0
    }
    pub const fn to_paddr(self) -> usize {
        self.0 << PGBITS
    }
    pub fn to_addr(self) -> Addr {
        Addr::from_paddr(self.to_paddr())
    }
}

impl fmt::Display for PPN {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PPN(0x{:x})", self.0)
    }
}

/// Single page-table entry. Carries the source pointer so `write_back`
/// can mutate the in-memory slot after a read-modify-write.
#[derive(Debug, Clone, Copy)]
pub struct PTE {
    bits: u64,
    ptr: Option<NonNull<u64>>,
}

/// PPN bits [47:12] (36-bit PPN, PALEN=48).
const PPN_SHIFT: u64 = PGBITS as u64;
const PPN_BITS: u64 = 36;
const PPN_MASK_IN_PTE: u64 = ((1u64 << PPN_BITS) - 1) << PPN_SHIFT;
/// Mask of every bit `PTEFlags` claims, used to strip flags before writing
/// a fresh PPN.
const FLAG_BITS_MASK: u64 = PTEFlags::all().bits();

impl PTE {
    pub fn from_ptr(ptr: NonNull<u64>) -> Self {
        let bits = unsafe { ptr.as_ptr().read_volatile() };
        Self { bits, ptr: Some(ptr) }
    }

    pub fn from_raw_ptr(ptr: *mut u64) -> Self {
        Self::from_ptr(NonNull::new(ptr).expect("PTE pointer must not be null"))
    }

    pub const fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits & FLAG_BITS_MASK)
    }

    /// Raw 64-bit PTE image. Useful for trace prints.
    #[allow(dead_code)]
    pub const fn image(&self) -> u64 {
        self.bits
    }

    pub fn set_flags(&mut self, flags: PTEFlags) -> &mut Self {
        self.bits = (self.bits & !FLAG_BITS_MASK) | flags.bits();
        self
    }

    pub fn ppn(self) -> PPN {
        PPN(((self.bits & PPN_MASK_IN_PTE) >> PPN_SHIFT) as usize)
    }

    pub fn set_ppn(&mut self, ppn: PPN) -> &mut Self {
        let ppn_bits = ((ppn.value() as u64) << PPN_SHIFT) & PPN_MASK_IN_PTE;
        self.bits = (self.bits & !PPN_MASK_IN_PTE) | ppn_bits;
        self
    }

    pub fn next_level(&self) -> PTETable {
        debug_assert!(self.ppn().value() != 0);
        PTETable::new(self.ppn().to_addr().ptr())
    }

    pub fn write_back(&self) -> Result<(), ()> {
        match self.ptr {
            Some(ptr) => {
                unsafe { ptr.as_ptr().write_volatile(self.bits) };
                Ok(())
            }
            None => Err(()),
        }
    }

    pub const fn is_valid(self) -> bool {
        (self.bits & PTEFlags::V.bits()) != 0
    }
}

impl fmt::Display for PTE {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PTE(0x{:016x}, {})", self.bits, self.ppn())
    }
}

pub struct PTETable {
    base: *mut u64,
}

impl PTETable {
    pub fn new(base: *mut u64) -> Self {
        debug_assert!(!base.is_null());
        Self { base }
    }

    pub fn get(&self, index: usize) -> PTE {
        PTE::from_raw_ptr(unsafe { self.base.add(index) })
    }

    pub fn set(&mut self, index: usize, pte: PTE) {
        unsafe { self.base.add(index).write_volatile(pte.bits) };
    }

    pub fn free(&self) {
        mm::page::free(self.base as usize);
    }
}

pub struct PageTable {
    pub root: usize,
}

impl PageTable {
    pub const fn new() -> Self {
        Self { root: 0 }
    }

    pub fn new_user() -> Self {
        let mut pagetable = Self::new();
        pagetable.create();
        pagetable
    }

    #[allow(dead_code)]
    pub fn from_root(root: usize) -> Self {
        debug_assert!(root != 0, "PageTable root cannot be zero");
        Self { root }
    }

    pub fn create(&mut self) {
        debug_assert!(self.root == 0, "PageTable::create called twice");
        self.root = mm::page::alloc_zero();
    }

    pub fn get_pgd(&self) -> usize {
        debug_assert!(self.root != 0);
        kaddr_to_paddr(self.root) & !PGMASK
    }

    pub fn is_mapped(&self, uaddr: usize) -> bool {
        self.find_pte(uaddr).is_some()
    }

    pub fn mapped_flag(&self, uaddr: usize) -> Option<PTEFlags> {
        self.find_pte(uaddr).map(|pte| pte.flags())
    }

    pub fn mapped_perm(&self, uaddr: usize) -> Option<MapPerm> {
        self.find_pte(uaddr).map(|pte| pte.flags().into())
    }

    /// Software Accessed emulation (PTEFlags::P stands in).
    pub fn mark_page_accessed(&mut self, uaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            let flags = pte.flags();
            if !flags.contains(PTEFlags::P) {
                pte.set_flags(flags | PTEFlags::P);
                pte.write_back()
                    .expect("Failed to write back PTE when marking page accessed");
                return true;
            }
        }
        false
    }

    pub fn mark_page_accessed_and_dirty(&mut self, uaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            let flags = pte.flags();
            if !flags.contains(PTEFlags::D) || !flags.contains(PTEFlags::P) {
                pte.set_flags(flags | PTEFlags::D | PTEFlags::P);
                pte.write_back()
                    .expect("Failed to write back PTE when marking page dirty");
                return true;
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn mmap_kernel(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::P | PTEFlags::D | PTEFlags::G;

        let mut pte = self.find_pte_or_create(kaddr);
        pte.set_flags(flags);
        pte.set_ppn(PPN::from_paddr(paddr));
        pte.write_back().expect("Failed to write back kernel PTE");
    }

    // ---------------- internal walker ----------------

    fn find_pte(&self, vaddr: usize) -> Option<PTE> {
        debug_assert!(self.root != 0);
        let vpns = Addr::from_vaddr(vaddr).vpn();

        let mut ptetable = PTETable::new(self.root as *mut u64);

        for (level, vpn) in vpns.iter().enumerate() {
            let pte = ptetable.get(*vpn);

            if level == LEAF_LEVEL {
                return if pte.is_valid() { Some(pte) } else { None };
            }

            // Intermediate slot: non-zero PPN means "installed".
            if pte.ppn().value() == 0 {
                return None;
            }

            ptetable = pte.next_level();
        }

        unreachable!("find_pte walker should always return within the loop")
    }

    fn find_pte_or_create(&mut self, vaddr: usize) -> PTE {
        debug_assert!(self.root != 0);
        let vpns = Addr::from_vaddr(vaddr).vpn();

        let mut ptetable = PTETable::new(self.root as *mut u64);

        for level in 0..PAGE_TABLE_LEVELS {
            let mut pte = ptetable.get(vpns[level]);

            if level == LEAF_LEVEL {
                return pte;
            }

            if pte.ppn().value() == 0 {
                let new_table_kaddr = mm::page::alloc_zero();
                let child_pa = kaddr_to_paddr(new_table_kaddr);
                debug_assert!(child_pa & PGMASK == 0, "child table PA must be page-aligned");
                pte.set_flags(PTEFlags::empty());
                pte.set_ppn(PPN::from_paddr(child_pa));
                ptetable.set(vpns[level], pte);
            }

            ptetable = pte.next_level();
        }

        unreachable!("find_pte_or_create walker should always return within the loop")
    }

    fn free_pagetable(&mut self, ptetable: &PTETable, level: usize) {
        if level != LEAF_LEVEL {
            for i in 0..ENTRIES_PER_TABLE {
                let pte = ptetable.get(i);
                if pte.ppn().value() != 0 {
                    self.free_pagetable(&pte.next_level(), level + 1);
                }
            }
        }

        ptetable.free();
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        if self.root == 0 {
            return;
        }
        let root_tbl = PTETable::new(self.root as *mut u64);
        self.free_pagetable(&root_tbl, 0);
        self.root = 0;
    }
}

unsafe impl Send for PageTable {}
unsafe impl Sync for PageTable {}

impl PageTableTrait for PageTable {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::P;
        if perm.contains(MapPerm::W) {
            flags |= PTEFlags::D;
        }

        let mut pte = self.find_pte_or_create(uaddr);
        debug_assert!(
            !pte.is_valid(),
            "PTE should NOT be valid before mmap, uaddr={:#x}, kaddr={:#x}",
            uaddr,
            kaddr
        );

        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE on mmap");
    }

    // fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
    //     let mut flags: PTEFlags = perm.into();
    //     flags |= PTEFlags::P;
    //     if perm.contains(MapPerm::W) {
    //         flags |= PTEFlags::D;
    //     }

    //     let mut pte = self.find_pte_or_create(kaddr);
    //     pte.set_flags(flags);
    //     pte.set_ppn(PPN::from_paddr(paddr));
    //     pte.write_back().expect("Failed to write back PTE on mmap_paddr");
    // }

    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::P;
        if perm.contains(MapPerm::W) {
            flags |= PTEFlags::D;
        }

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE on mmap_replace");
    }

    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::P;
        if perm.contains(MapPerm::W) {
            flags |= PTEFlags::D;
        }

        if let Some(mut pte) = self.find_pte(uaddr) {
            pte.set_flags(flags);
            pte.write_back().expect("Failed to write back PTE on mmap_replace_perm");
        }
    }

    fn munmap(&mut self, uaddr: usize) -> Result<(), ()> {
        if let Some(mut pte) = self.find_pte(uaddr) {
            pte.set_flags(PTEFlags::empty());
            pte.write_back().expect("Failed to write back PTE on munmap");
            Ok(())
        } else {
            Err(())
        }
    }

    fn munmap_with_check(&mut self, uaddr: usize, expected_kaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            if pte.ppn().to_addr().kaddr() == expected_kaddr {
                pte.set_flags(PTEFlags::empty())
                    .write_back()
                    .expect("Failed to write back PTE on munmap_with_check");
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)> {
        self.find_pte(uaddr).map(|mut pte| {
            let flags = pte.flags();
            let accessed = flags.contains(PTEFlags::P);
            let dirty = flags.contains(PTEFlags::D);
            pte.set_flags(flags.difference(PTEFlags::P | PTEFlags::D))
                .write_back()
                .expect("Failed to write back PTE on take_access_dirty_bit");
            (accessed, dirty)
        })
    }
}
