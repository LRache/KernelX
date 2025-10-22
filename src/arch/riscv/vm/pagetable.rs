use crate::kernel::mm::MapPerm;
use crate::kernel::mm;
use crate::arch::riscv::PGBITS;
use crate::arch::PageTableTrait;

use super::pte::{Addr, PTE, PTEFlags, PTETable};

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;

pub trait PageAllocator {
    fn alloc_page() -> usize;
}

unsafe extern "C"{
    static __trampoline_start: usize;
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

    pub fn apply(&self) {
        unsafe {
            core::arch::asm!(
                "sfence.vma",
                "csrw satp, {}",
                "sfence.vma",
                in(reg) self.get_satp(),
            );
        }
    }

    fn find_pte(&self, vaddr: usize) -> Option<PTE> {        
        self.find_pte_vpn(Addr::from_vaddr(vaddr).vpn())
    }

    fn find_pte_vpn(&self, vpn: [usize; PAGE_TABLE_LEVELS]) -> Option<PTE> {
        debug_assert!(self.root != 0);
        let mut ptetable = PTETable::new(self.root as *mut usize);
        
        for level in 0..PAGE_TABLE_LEVELS {
            let pte = ptetable.get(vpn[level]);
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
                let page = mm::page::alloc_zero();
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
                let ppn = pte.ppn();
                assert!(ppn.value() << PGBITS <= 0x88000000, "PPN out of range: 0x{:x}, level={}, i={}", ppn.value() << PGBITS, level, i);
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

    pub fn is_mapped(&self, uaddr: usize) -> bool {
        self.find_pte(uaddr).is_some()
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
        let flags = perm.into();

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

    fn munmap(&mut self, vaddr: usize) {
        let mut pte = self.find_pte(vaddr).expect("PTE not found for munmap");
        pte.set_flags(PTEFlags::empty());
        pte.write_back().expect("Failed to write back PTE for munmap");
    }
}

pub struct NormalPageAllocator;

impl PageAllocator for NormalPageAllocator {
    fn alloc_page() -> usize {
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
