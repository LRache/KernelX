use crate::arch::{PageTableTrait, flush_tlb_cpu_mask};
use crate::kernel::mm;
use crate::kernel::mm::MapPerm;
use core::sync::atomic::{Ordering, fence};

use super::kernelpagetable::{install_shared_kernel_mappings, is_shared_kernel_root};
use super::pte::{Addr, PTE, PTEFlags, PTETable};

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;
pub(super) const ENTRIES_PER_TABLE: usize = 512;

pub trait PageAllocator {
    fn alloc_zero() -> usize;
}

pub struct PageTableImpls<T: PageAllocator> {
    pub root: usize,
    has_shared_kernel_mappings: bool,
    /// CPUs that may use this page table again without first performing a
    /// local TLB invalidation. Protected by the owning page-table lock.
    active_cpu_mask: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: PageAllocator> PageTableImpls<T> {
    pub fn create(&mut self) {
        debug_assert!(
            self.root == 0,
            "PageTable root should be zero when creating a new PageTable"
        );

        self.root = mm::page::alloc_zero();
    }

    pub fn from_root(root: usize) -> Self {
        debug_assert!(root != 0, "PageTable root cannot be zero");
        Self {
            root,
            has_shared_kernel_mappings: false,
            active_cpu_mask: 0,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn find_pte(&self, vaddr: usize) -> Option<PTE> {
        self.find_pte_vpn(Addr::from_vaddr(vaddr).vpn())
    }

    fn find_pte_vpn(&self, vpns: [usize; PAGE_TABLE_LEVELS]) -> Option<PTE> {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);

        for (level, vpn) in vpns.iter().enumerate() {
            let pte = ptetable.get(*vpn);
            if !pte.is_valid() {
                return None;
            }

            if pte.is_leaf() || level == LEAF_LEVEL {
                return Some(pte);
            }

            ptetable = pte.next_level();
        }

        unreachable!("Page table traversal should always return before this point")
    }

    /// find pte or create a new one if it doesn't exist
    fn find_pte_or_create(&mut self, vaddr: usize) -> PTE {
        self.find_pte_or_create_vpn(Addr::from_vaddr(vaddr).vpn())
    }

    fn find_pte_or_create_vpn(&mut self, vpn: [usize; PAGE_TABLE_LEVELS]) -> PTE {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);

        for level in 0..PAGE_TABLE_LEVELS {
            let mut pte = ptetable.get(vpn[level]);

            if level == LEAF_LEVEL {
                return pte;
            }

            if !pte.is_valid() {
                // Create a new page table entry
                let page = T::alloc_zero();
                let paddr = Addr::from_kaddr(page);
                pte.set_ppn(paddr.ppn());
                pte.set_flags(PTEFlags::V);
                ptetable.set(vpn[level], pte);
            } else if pte.is_leaf() {
                self.split_leaf(&mut pte, level);
            }

            ptetable = pte.next_level();
        }

        unreachable!("Page table traversal should always return before this point")
    }

    fn find_pte_or_split(&mut self, vaddr: usize) -> Option<PTE> {
        debug_assert!(self.root != 0);
        let vpn = Addr::from_vaddr(vaddr).vpn();
        let mut ptetable = PTETable::new(self.root as *mut usize);

        for level in 0..PAGE_TABLE_LEVELS {
            let mut pte = ptetable.get(vpn[level]);
            if !pte.is_valid() {
                return None;
            }
            if level == LEAF_LEVEL {
                return Some(pte);
            }
            if pte.is_leaf() {
                self.split_leaf(&mut pte, level);
            }
            ptetable = pte.next_level();
        }

        unreachable!("Page table traversal should always return before this point")
    }

    fn split_leaf(&mut self, pte: &mut PTE, level: usize) {
        debug_assert!(level < LEAF_LEVEL);
        debug_assert!(pte.is_leaf());

        let child_page = T::alloc_zero();
        let mut child_table = PTETable::new(child_page as *mut usize);
        let child_ppn_step = 1 << ((LEAF_LEVEL - level - 1) * 9);
        let base_ppn = pte.ppn().value();
        let flags = pte.flags();

        for index in 0..ENTRIES_PER_TABLE {
            let mut child = child_table.get(index);
            child.set_ppn(super::pte::PPN::new(base_ppn + index * child_ppn_step));
            child.set_flags(flags);
            child_table.set(index, child);
        }

        fence(Ordering::Release);
        pte.set_ppn(Addr::from_kaddr(child_page).ppn());
        pte.set_flags(PTEFlags::V);
        pte.write_back().expect("Failed to replace huge leaf with a page table");
    }

    pub(super) fn ensure_root_entry(&mut self, index: usize) {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);
        let mut pte = ptetable.get(index);

        if pte.is_valid() {
            debug_assert!(
                !pte.flags().intersects(PTEFlags::R | PTEFlags::W | PTEFlags::X),
                "shared kernel root entry should be a non-leaf PTE"
            );
            return;
        }

        let page = T::alloc_zero();
        pte.set_ppn(Addr::from_kaddr(page).ppn());
        pte.set_flags(PTEFlags::V);
        ptetable.set(index, pte);
    }

    fn free_pagetable(&mut self, ptetable: &PTETable, level: usize) {
        if level != LEAF_LEVEL {
            for i in 0..ENTRIES_PER_TABLE {
                if level == 0 && self.has_shared_kernel_mappings && is_shared_kernel_root(i) {
                    continue;
                }

                let pte = ptetable.get(i);
                if pte.is_valid() && !pte.is_leaf() {
                    self.free_pagetable(&pte.next_level(), level + 1);
                }
            }
        }

        ptetable.free();
    }

    pub fn get_satp(&self) -> usize {
        const MODE_SV39: usize = 8;
        let ppn = Addr::new(self.root as *const u8).ppn().value();
        (MODE_SV39 << 60) | ppn
    }

    pub fn activate_cpu(&mut self, cpu_id: usize) {
        self.active_cpu_mask |= 1usize
            .checked_shl(cpu_id.try_into().expect("CPU ID does not fit in u32"))
            .expect("CPU ID exceeds page-table CPU mask width");
    }

    pub fn deactivate_cpu(&mut self, cpu_id: usize) {
        self.active_cpu_mask &= !1usize
            .checked_shl(cpu_id.try_into().expect("CPU ID does not fit in u32"))
            .expect("CPU ID exceeds page-table CPU mask width");
    }

    pub fn active_cpu_mask(&self) -> usize {
        self.active_cpu_mask
    }

    pub fn flush_tlb(&self) {
        flush_tlb_cpu_mask(self.active_cpu_mask);
    }

    #[allow(dead_code)]
    pub fn is_mapped(&self, uaddr: usize) -> bool {
        self.find_pte(uaddr).is_some()
    }

    pub fn mapped_flag(&self, uaddr: usize) -> Option<PTEFlags> {
        self.find_pte(uaddr).map(|pte| pte.flags())
    }

    pub fn mmap_kernel(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let mut flags = perm.into();
        flags = flags | PTEFlags::G | PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(kaddr);

        pte.set_flags(flags);
        pte.set_ppn(Addr::from_paddr(paddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    pub(crate) fn munmap_no_flush(&mut self, vaddr: usize) -> Result<(), ()> {
        let mut pte = self.find_pte_or_split(vaddr).ok_or(())?;
        pte.set_flags(PTEFlags::empty());
        pte.write_back().expect("Failed to write back PTE for munmap");
        Ok(())
    }

    pub(crate) fn mmap_replace_perm_no_flush(&mut self, uaddr: usize, perm: MapPerm) -> bool {
        let flags = perm.into();

        let Some(mut pte) = self.find_pte(uaddr) else {
            return false;
        };
        pte.set_flags(flags);
        pte.write_back().expect("Failed to write back PTE");
        true
    }

    pub(crate) fn mmap_replace_perm_with_check_and_ad_no_flush(
        &mut self,
        uaddr: usize,
        expected_kaddr: usize,
        perm: MapPerm,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(uaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let old_flags = pte.flags();
        pte.set_flags(perm.into())
            .write_back()
            .expect("Failed to write back PTE on checked mmap_replace_perm");
        Some((old_flags.contains(PTEFlags::A), old_flags.contains(PTEFlags::D)))
    }

    pub(crate) fn munmap_with_check_no_flush(&mut self, uaddr: usize, expected_kaddr: usize) -> bool {
        let Some(mut pte) = self.find_pte(uaddr) else {
            return false;
        };
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return false;
        }

        pte.set_flags(PTEFlags::empty())
            .write_back()
            .expect("Failed to write back PTE for munmap_with_check");
        true
    }

    pub(crate) fn munmap_with_check_and_ad_no_flush(
        &mut self,
        uaddr: usize,
        expected_kaddr: usize,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(uaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let flags = pte.flags();
        pte.set_flags(PTEFlags::empty())
            .write_back()
            .expect("Failed to write back PTE for munmap_with_check_and_ad");
        Some((flags.contains(PTEFlags::A), flags.contains(PTEFlags::D)))
    }

    // pub fn mapped_page(&self, uaddr: usize) -> Option<MappedPage<'_>> {
    //     if let Some(pte) = self.find_pte(uaddr) {
    //         Some(MappedPage { pte, _marker: core::marker::PhantomData })
    //     } else {
    //         None
    //     }
    // }

    pub fn mark_page_accessed(&mut self, uaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            let flags = pte.flags();
            if !flags.contains(PTEFlags::A) {
                pte.set_flags(flags | PTEFlags::A);
                pte.write_back()
                    .expect("Failed to write back PTE when marking page accessed");
                self.flush_tlb();
                return true;
            }
        }
        false
    }

    pub fn mark_page_accessed_and_dirty(&mut self, uaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            let flags = pte.flags();
            if !flags.contains(PTEFlags::D) || !flags.contains(PTEFlags::A) {
                pte.set_flags(flags | PTEFlags::D | PTEFlags::A);
                pte.write_back()
                    .expect("Failed to write back PTE when marking page dirty");
                self.flush_tlb();
                return true;
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn mapped_perm(&self, uaddr: usize) -> Option<MapPerm> {
        self.find_pte(uaddr).map(|pte| pte.flags().into())
    }
}

impl<T: PageAllocator> Drop for PageTableImpls<T> {
    fn drop(&mut self) {
        if self.root == 0 {
            return;
        }

        self.free_pagetable(&PTETable::new(self.root as *mut usize), 0);
        self.root = 0; // Clear the root pointer to avoid double free
    }
}

unsafe impl<T: PageAllocator> Send for PageTableImpls<T> {}
unsafe impl<T: PageAllocator> Sync for PageTableImpls<T> {}

impl<T: PageAllocator> PageTableTrait for PageTableImpls<T> {
    unsafe fn mmap_raw(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags = perm.into();

        flags |= PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(uaddr);
        debug_assert!(
            !pte.is_valid(),
            "PTE should NOT be valid before mmap, uaddr= {:#x}, kaddr = {:#x}",
            uaddr,
            kaddr
        );

        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
        self.flush_tlb();
    }

    // fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
    //     let flags = perm.into();

    //     let mut pte = self.find_pte_or_create(kaddr);
    //     pte.set_flags(flags);
    //     pte.set_ppn(Addr::from_paddr(paddr).ppn());
    //     pte.write_back().expect("Failed to write back PTE");
    // }

    unsafe fn mmap_replace_raw(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let flags = perm.into();

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
        self.flush_tlb();
    }

    fn mmap_replace_with_check_and_ad(
        &mut self,
        uaddr: usize,
        expected_kaddr: usize,
        replacement_kaddr: usize,
        perm: MapPerm,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(uaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let old_flags = pte.flags();
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::A | PTEFlags::D;
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(replacement_kaddr).ppn());
        pte.write_back()
            .expect("Failed to write back PTE on checked mmap_replace");
        self.flush_tlb();
        Some((old_flags.contains(PTEFlags::A), old_flags.contains(PTEFlags::D)))
    }

    // fn mmap_replace_kaddr(&mut self, uaddr: usize, kaddr: usize) {
    //     let mut pte = self.find_pte_or_create(uaddr);
    //     pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
    //     pte.write_back().expect("Failed to write back PTE");
    // }

    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm) {
        if self.mmap_replace_perm_no_flush(uaddr, perm) {
            self.flush_tlb();
        }
    }

    fn mmap_replace_perm_with_check_and_ad(
        &mut self,
        uaddr: usize,
        expected_kaddr: usize,
        perm: MapPerm,
    ) -> Option<(bool, bool)> {
        let access_dirty = self.mmap_replace_perm_with_check_and_ad_no_flush(uaddr, expected_kaddr, perm)?;
        self.flush_tlb();
        Some(access_dirty)
    }

    fn munmap_raw(&mut self, vaddr: usize) -> Result<(), ()> {
        self.munmap_no_flush(vaddr)?;
        self.flush_tlb();
        Ok(())
    }

    fn munmap_with_check(&mut self, uaddr: usize, kaddr: usize) -> bool {
        // Using atomic operation is unnecessary here because the page table is
        // write-locked during munmap_with_check.
        if self.munmap_with_check_no_flush(uaddr, kaddr) {
            self.flush_tlb();
            true
        } else {
            false
        }
    }

    fn munmap_with_check_and_ad(&mut self, uaddr: usize, expected_kaddr: usize) -> Option<(bool, bool)> {
        let access_dirty = self.munmap_with_check_and_ad_no_flush(uaddr, expected_kaddr)?;
        self.flush_tlb();
        Some(access_dirty)
    }

    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)> {
        self.find_pte(uaddr).map(|mut pte| {
            let flags = pte.flags();
            let accessed = flags.contains(PTEFlags::A);
            let dirty = flags.contains(PTEFlags::D);
            pte.set_flags(flags.difference(PTEFlags::A | PTEFlags::D))
                .write_back()
                .expect("Failed to write back PTE when taking access and dirty bits");
            self.flush_tlb();
            (accessed, dirty)
        })
    }

    fn take_access_dirty_bit_with_check_no_flush(
        &mut self,
        uaddr: usize,
        expected_kaddr: usize,
    ) -> Option<(bool, bool)> {
        let mut pte = self.find_pte(uaddr)?;
        if pte.ppn().to_addr().kaddr() != expected_kaddr {
            return None;
        }

        let flags = pte.flags();
        let accessed = flags.contains(PTEFlags::A);
        let dirty = flags.contains(PTEFlags::D);
        pte.set_flags(flags.difference(PTEFlags::A | PTEFlags::D))
            .write_back()
            .expect("Failed to write back PTE when taking checked access and dirty bits");
        Some((accessed, dirty))
    }

    // fn mapped_page(&self, uaddr: usize) -> Option<MappedPage> {
    //     if let Some(pte) = self.find_pte(uaddr) {
    //         let kaddr = pte.ppn().to_addr().kaddr();
    //         let perm: MapPerm = pte.flags().into();
    //         Some(crate::arch::arch::MappedPage { kaddr, perm })
    //     } else {
    //         None
    //     }
    // }
}

pub struct NormalPageAllocator;

impl PageAllocator for NormalPageAllocator {
    fn alloc_zero() -> usize {
        mm::page::alloc_zero()
    }
}

pub type PageTable = PageTableImpls<NormalPageAllocator>;

impl PageTable {
    pub const fn new() -> Self {
        Self {
            root: 0,
            has_shared_kernel_mappings: false,
            active_cpu_mask: 0,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn new_user() -> Self {
        let mut pagetable = Self::new();
        pagetable.create();
        install_shared_kernel_mappings(&mut pagetable);
        pagetable.has_shared_kernel_mappings = true;
        pagetable
    }
}
