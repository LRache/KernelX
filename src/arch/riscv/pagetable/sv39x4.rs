use crate::arch::PageTableTrait;
use crate::kernel::mm;
use crate::kernel::mm::MapPerm;

use super::pte::{Addr, PTE, PTEFlags, PTETable};

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;
const ROOT_PAGES: usize = 4;
const ROOT_ENTRIES: usize = ROOT_PAGES * 512;
const ROOT_ALIGN: usize = ROOT_PAGES * crate::arch::PGSIZE;
const GPA_BITS: usize = 41;

pub struct Sv39x4PageTable {
    root: usize,
}

impl Sv39x4PageTable {
    pub const fn new() -> Self {
        Self { root: 0 }
    }

    pub fn create(&mut self) {
        debug_assert!(
            self.root == 0,
            "Sv39x4PageTable root should be zero when creating a new page table"
        );

        self.root = mm::page::alloc_contiguous_aligned(ROOT_PAGES, ROOT_ALIGN);
        for page in 0..ROOT_PAGES {
            mm::page::zero(self.root + page * crate::arch::PGSIZE);
        }
    }

    pub fn get_hgatp(&self) -> usize {
        debug_assert!(self.root != 0, "Sv39x4PageTable root is not initialized");

        const MODE_SV39X4: usize = 8;
        let ppn = Addr::new(self.root as *const u8).ppn().value();
        debug_assert!(ppn & (ROOT_PAGES - 1) == 0, "Sv39x4 root must be 16 KiB aligned");
        (MODE_SV39X4 << 60) | ppn
    }

    pub fn is_mapped(&self, gaddr: usize) -> bool {
        self.find_pte(gaddr).is_some()
    }

    fn gpn(gaddr: usize) -> [usize; PAGE_TABLE_LEVELS] {
        debug_assert!(gaddr < (1 << GPA_BITS), "guest physical address exceeds Sv39x4 range");
        [(gaddr >> 30) & 0x7ff, (gaddr >> 21) & 0x1ff, (gaddr >> 12) & 0x1ff]
    }

    fn find_pte(&self, gaddr: usize) -> Option<PTE> {
        self.find_pte_gpn(Self::gpn(gaddr))
    }

    fn find_pte_gpn(&self, gpns: [usize; PAGE_TABLE_LEVELS]) -> Option<PTE> {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);

        for (level, gpn) in gpns.iter().enumerate() {
            let pte = ptetable.get(*gpn);
            if !pte.is_valid() {
                return None;
            }

            if level == LEAF_LEVEL {
                return Some(pte);
            }

            ptetable = pte.next_level();
        }

        unreachable!("page table traversal should return before this point")
    }

    fn find_pte_or_create(&mut self, gaddr: usize) -> PTE {
        self.find_pte_or_create_gpn(Self::gpn(gaddr))
    }

    fn find_pte_or_create_gpn(&mut self, gpn: [usize; PAGE_TABLE_LEVELS]) -> PTE {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);

        for level in 0..PAGE_TABLE_LEVELS {
            let mut pte = ptetable.get(gpn[level]);

            if level == LEAF_LEVEL {
                return pte;
            }

            if !pte.is_valid() {
                let page = mm::page::alloc_zero();
                let paddr = Addr::from_kaddr(page);
                pte.set_ppn(paddr.ppn());
                pte.set_flags(PTEFlags::V);
                ptetable.set(gpn[level], pte);
            }

            ptetable = pte.next_level();
        }

        unreachable!("page table traversal should return before this point")
    }

    fn free_pagetable(&mut self, ptetable: &PTETable, level: usize, entries: usize) {
        if level != LEAF_LEVEL {
            for i in 0..entries {
                let pte = ptetable.get(i);
                if pte.is_valid() {
                    self.free_pagetable(&pte.next_level(), level + 1, 512);
                }
            }
        }

        if level == 0 {
            mm::page::free_contiguous_aligned(self.root, ROOT_PAGES, ROOT_ALIGN);
        } else {
            ptetable.free();
        }
    }

    pub fn munmap_if_mapped(&mut self, gaddr: usize) -> Option<usize> {
        self.find_pte(gaddr).map(|mut pte| {
            let kaddr = pte.ppn().to_addr().kaddr();
            pte.set_flags(PTEFlags::empty())
                .write_back()
                .expect("Failed to write back Sv39x4 PTE for munmap_if_mapped");
            kaddr
        })
    }

    pub fn mapped_perm(&self, gaddr: usize) -> Option<MapPerm> {
        self.find_pte(gaddr).map(|pte| pte.flags().into())
    }
}

impl PageTableTrait for Sv39x4PageTable {
    unsafe fn mmap_raw(&mut self, gaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::U | PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(gaddr);
        debug_assert!(
            !pte.is_valid(),
            "PTE should not be valid before mmap, gaddr={:#x}, kaddr={:#x}",
            gaddr,
            kaddr
        );

        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back Sv39x4 PTE");
    }

    unsafe fn mmap_replace_raw(&mut self, gaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(gaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back Sv39x4 PTE");
    }

    fn mmap_replace_with_check_and_ad(
        &mut self,
        gaddr: usize,
        expected_kaddr: usize,
        replacement_kaddr: usize,
        perm: MapPerm,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(gaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let old_flags = pte.flags();
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::U | PTEFlags::A | PTEFlags::D;
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(replacement_kaddr).ppn());
        pte.write_back()
            .expect("Failed to write back Sv39x4 PTE on checked mmap_replace");
        Some((old_flags.contains(PTEFlags::A), old_flags.contains(PTEFlags::D)))
    }

    fn mmap_replace_perm(&mut self, gaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::A | PTEFlags::D;

        if let Some(mut pte) = self.find_pte(gaddr) {
            pte.set_flags(flags);
            pte.write_back().expect("Failed to write back Sv39x4 PTE");
        }
    }

    fn mmap_replace_perm_with_check_and_ad(
        &mut self,
        gaddr: usize,
        expected_kaddr: usize,
        perm: MapPerm,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(gaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let old_flags = pte.flags();
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::A | PTEFlags::D;
        pte.set_flags(flags)
            .write_back()
            .expect("Failed to write back Sv39x4 PTE on checked mmap_replace_perm");
        Some((old_flags.contains(PTEFlags::A), old_flags.contains(PTEFlags::D)))
    }

    fn munmap_raw(&mut self, gaddr: usize) -> Result<(), ()> {
        if let Some(mut pte) = self.find_pte(gaddr) {
            pte.set_flags(PTEFlags::empty());
            pte.write_back().expect("Failed to write back Sv39x4 PTE for munmap");
            Ok(())
        } else {
            Err(())
        }
    }

    fn munmap_with_check(&mut self, gaddr: usize, expected_kaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(gaddr) {
            if pte.ppn().to_addr().kaddr() == expected_kaddr {
                pte.set_flags(PTEFlags::empty())
                    .write_back()
                    .expect("Failed to write back Sv39x4 PTE for munmap_with_check");
                return true;
            }
        }
        false
    }

    fn munmap_with_check_and_ad(&mut self, gaddr: usize, expected_kaddr: usize) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(gaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let flags = pte.flags();
        let accessed = flags.contains(PTEFlags::A);
        let dirty = flags.contains(PTEFlags::D);
        pte.set_flags(PTEFlags::empty())
            .write_back()
            .expect("Failed to write back Sv39x4 PTE for munmap_with_check_and_ad");
        Some((accessed, dirty))
    }

    fn take_access_dirty_bit(&mut self, gaddr: usize) -> Option<(bool, bool)> {
        self.find_pte(gaddr).map(|mut pte| {
            let flags = pte.flags();
            let accessed = flags.contains(PTEFlags::A);
            let dirty = flags.contains(PTEFlags::D);
            pte.set_flags(flags.difference(PTEFlags::A | PTEFlags::D))
                .write_back()
                .expect("Failed to write back Sv39x4 PTE when taking access and dirty bits");
            (accessed, dirty)
        })
    }

    fn take_access_dirty_bit_with_check_no_flush(
        &mut self,
        gaddr: usize,
        expected_kaddr: usize,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(gaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let flags = pte.flags();
        let accessed = flags.contains(PTEFlags::A);
        let dirty = flags.contains(PTEFlags::D);
        pte.set_flags(flags.difference(PTEFlags::A | PTEFlags::D))
            .write_back()
            .expect("Failed to write back Sv39x4 PTE when taking checked access and dirty bits");
        Some((accessed, dirty))
    }
}

impl Drop for Sv39x4PageTable {
    fn drop(&mut self) {
        if self.root != 0 {
            self.free_pagetable(&PTETable::new(self.root as *mut usize), 0, ROOT_ENTRIES);
            self.root = 0;
        }
    }
}

unsafe impl Send for Sv39x4PageTable {}
unsafe impl Sync for Sv39x4PageTable {}
