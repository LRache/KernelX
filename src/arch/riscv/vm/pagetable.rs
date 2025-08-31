use crate::arch::riscv::pte::{Addr, PTE, PTEFlags, PTETable};
use crate::arch::{PageTableTrait, PGBITS};
use crate::arch;
use crate::kernel::mm::MapPerm;
use crate::kernel::mm;
use crate::kernel::errno::Errno;

const PAGE_TABLE_LEVELS: usize = 3;
const LEAF_LEVEL: usize = 2;

unsafe extern "C"{
    static __trampoline_start: usize;
}

fn perm2flag(perm: MapPerm) -> PTEFlags {
    let mut flags = PTEFlags::V;
    if perm.contains(MapPerm::R) { flags.insert(PTEFlags::R); }
    if perm.contains(MapPerm::W) { flags.insert(PTEFlags::W); }
    if perm.contains(MapPerm::X) { flags.insert(PTEFlags::X); }
    if perm.contains(MapPerm::U) { flags.insert(PTEFlags::U); }
    flags
}

pub struct PageTable {
    pub root: usize,
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable {
            root: 0,
        }
    }

    pub fn create(&mut self) {
        if self.root != 0 {
            panic!("PageTable root already set");
        }
        
        self.root = mm::page::alloc_zero();
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
                pte.write_back().expect("Failed to write back PTE");
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
}

impl Drop for PageTable {
    fn drop(&mut self) {
        self.free_pagetable(&PTETable::new(self.root as *mut usize), 0);
        self.root = 0; // Clear the root pointer to avoid double free
    }
}

unsafe impl Send for PageTable {}
unsafe impl Sync for PageTable {}

impl PageTableTrait for PageTable {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let flags = perm2flag(perm);

        let mut pte = self.find_pte_or_create(uaddr);
        assert!(!pte.is_valid(), "PTE should NOT be valid before mmap, uaddr= {:#x}, kaddr = {:#x}", uaddr, kaddr);
        
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let flags = perm2flag(perm);

        let mut pte = self.find_pte_or_create(kaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_paddr(paddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let flags = perm2flag(perm);

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back().expect("Failed to write back PTE");
    }

    fn munmap(&mut self, vaddr: usize) {
        let mut pte = self.find_pte(vaddr).expect("PTE not found for munmap");
        pte.set_flags(PTEFlags::empty());
        pte.write_back().expect("Failed to write back PTE for munmap");
        mm::page::free(pte.page() as usize);
    }

    fn munmap_if_mapped(&mut self, uaddr: usize) -> bool {
        if let Some(mut pte) = self.find_pte(uaddr) {
            pte.set_flags(PTEFlags::empty());
            pte.write_back().expect("Failed to write back PTE for munmap_if_mapped");
            mm::page::free(pte.page() as usize);
            return true;
        }
        false
    }

    fn is_mapped(&self, vaddr: usize) -> bool {
        if let Some(pte) = self.find_pte(vaddr) {
            pte.is_valid()
        } else {
            false
        }
    }

    fn translate(&self, vaddr: usize) -> Option<usize> {
        let pte = self.find_pte(vaddr & !arch::PGMASK)?;
        Some(pte.page() as usize + Addr::from_kaddr(vaddr).pgoff())
    }

    fn mprotect(&mut self, vaddr: usize, perm: MapPerm) -> Result<(), Errno> {
        assert!(vaddr & arch::PGMASK == 0, "vaddr must be page-aligned");

        let mut pte = self.find_pte(vaddr).ok_or(Errno::EINVAL)?;
        
        if !pte.is_valid() {
            return Err(Errno::EINVAL);
        }

        let flags = perm2flag(perm);

        pte.set_flags(flags);
        pte.write_back().map_err(|_| Errno::EIO)?;

        Ok(())
    }
}
