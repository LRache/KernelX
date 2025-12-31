use crate::{kernel::mm::MapPerm};
use crate::kernel::mm;
use crate::arch::PageTableTrait;

use super::pte::{Addr, PTE, PTEFlags, PTETable};

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;

pub trait PageAllocator {
    fn alloc_zero() -> usize;
}

pub struct MappedPage {
    pte: PTE,
}

impl MappedPage {
    pub fn page(&self) -> usize {
        self.pte.ppn().to_addr().kaddr()
    }

    pub fn perm(&self) -> MapPerm {
        self.pte.flags().into()
    }

    pub fn set_perm(&mut self, perm: MapPerm) {
        let flags: PTEFlags = perm.into();
        self.pte.set_flags(flags);
        self.pte.write_back().expect("Failed to write back PTE");
    }
}

pub struct PageTableImpls<T: PageAllocator> {
    pub root: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: PageAllocator> PageTableImpls<T> {
    pub fn create(&mut self) {
        debug_assert!(self.root == 0, "PageTable root should be zero when creating a new PageTable");
        
        self.root = mm::page::alloc_zero();
    }

    pub fn from_root(root: usize) -> Self {
        debug_assert!(root != 0, "PageTable root cannot be zero");
        Self {
            root,
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
            
            if level == LEAF_LEVEL {
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
            }

            ptetable = pte.next_level();
        }
        
        unreachable!("Page table traversal should always return before this point")
    }

    fn free_pagetable(&mut self, ptetable: &PTETable, level: usize) {
        if level != LEAF_LEVEL {
            for i in 0..512 {
                let pte = ptetable.get(i);
                if pte.is_valid() {
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

    #[allow(dead_code)]
    pub fn is_mapped(&self, uaddr: usize) -> bool {
        self.find_pte(uaddr).is_some()
    }

    pub fn mapped_flag(&self, uaddr: usize) -> Option<PTEFlags> {
        self.find_pte(uaddr).map(|pte| pte.flags())
    }

    pub fn mmap_kernel(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let mut flags = perm.into();
        flags = flags | PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(kaddr);
        
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_paddr(paddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
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
                pte.write_back().expect("Failed to write back PTE when marking page accessed");
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
                pte.write_back().expect("Failed to write back PTE when marking page dirty");
                return true;
            }
        }
        false
    }

    pub fn mapped_perm(&self, uaddr: usize) -> Option<MapPerm> {
        self.find_pte(uaddr).map(|pte| pte.flags().into())
    }
}

impl<T: PageAllocator> Drop for PageTableImpls<T> {
    fn drop(&mut self) {
        self.free_pagetable(&PTETable::new(self.root as *mut usize), 0);
        self.root = 0; // Clear the root pointer to avoid double free
    }
}

unsafe impl<T: PageAllocator> Send for PageTableImpls<T> {}
unsafe impl<T: PageAllocator> Sync for PageTableImpls<T> {}

impl<T: PageAllocator> PageTableTrait for PageTableImpls<T> {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags = perm.into();

        flags |= PTEFlags::A | PTEFlags::D;

        let mut pte = self.find_pte_or_create(uaddr);
        debug_assert!(!pte.is_valid(), "PTE should NOT be valid before mmap, uaddr= {:#x}, kaddr = {:#x}", uaddr, kaddr);
        
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let flags = perm.into();

        let mut pte = self.find_pte_or_create(kaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_paddr(paddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let flags = perm.into();

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }
    
    fn mmap_replace_kaddr(&mut self, uaddr: usize, kaddr: usize) {
        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm) {
        let flags = perm.into();

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.write_back().expect("Failed to write back PTE");
    }

    fn munmap(&mut self, vaddr: usize) {
        let mut pte = self.find_pte(vaddr).expect("PTE not found for munmap");
        pte.set_flags(PTEFlags::empty());
        pte.write_back().expect("Failed to write back PTE for munmap");
    }

    fn munmap_with_check(&mut self, uaddr: usize, kaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            // Using atomic opearation is unnessary here,
            // because pagetable is write-locked during munmap_with_check.
            if pte.ppn().to_addr().kaddr() == kaddr {
                pte.set_flags(PTEFlags::empty()).write_back().expect("Failed to write back PTE for munmap_with_check");
                return true;
            } else {
                return false;
            }
        } else {
            false
        }
    }

    fn take_access_dirty_bit(&mut self, uaddr: usize) -> Option<(bool, bool)> {
        self.find_pte(uaddr).map(|mut pte| {
            let flags = pte.flags();
            let accessed = flags.contains(PTEFlags::A);
            let dirty = flags.contains(PTEFlags::D);
            pte.set_flags(flags.difference(PTEFlags::A | PTEFlags::D)).write_back().expect("Failed to write back PTE when taking access and dirty bits");
            (accessed, dirty)
        })
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
            _marker: core::marker::PhantomData,
        }
    }
}
