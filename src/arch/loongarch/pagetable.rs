//! LoongArch64 page-table skeleton.
//!
//! Real page-walk logic lands in Phase 3. For now this exists only so that
//! (a) the type `PageTable` has the same shape as the RISC-V port and (b)
//! every method referenced from `src/kernel/mm/**` and
//! `src/arch/loongarch/arch.rs` compiles. Every method panics at runtime.

use bitflags::bitflags;

use crate::arch::PageTableTrait;
use crate::kernel::mm::MapPerm;

bitflags! {
    /// LoongArch PTE bit layout (PALEN=48, RPLV=0, huge=0).
    /// Bits here follow Vol.1 §8.5 and Linux `pgtable-bits.h`. Kept as a
    /// `u16` for parity with the RISC-V `PTEFlags` type — only the low bits
    /// are used by code that already compiles against both ports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PTEFlags: u16 {
        const V    = 1 << 0;  // valid
        const D    = 1 << 1;  // dirty
        const PLV0 = 0 << 2;
        const PLV3 = 3 << 2;  // user-accessible
        const SUC  = 0 << 4;  // strongly uncached (MAT=0)
        const CC   = 1 << 4;  // cached coherent (MAT=1)
        const WUC  = 2 << 4;  // weakly uncached (MAT=2)
        const G    = 1 << 6;  // global
        const P    = 1 << 7;  // present (software bit)
        const W    = 1 << 8;  // writable (software bit)
        const NR   = 1 << 9;  // not-readable (bit 61 in the HW PTE; mirrored here)
        const NX   = 1 << 10; // not-executable (bit 62 mirrored)
        const RPLV = 1 << 11; // restrict PLV (bit 63 mirrored)
    }
}

impl From<MapPerm> for PTEFlags {
    fn from(perm: MapPerm) -> Self {
        // Start from valid+present; apply R/W/X inversely for NR/NX.
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

/// Page-table root + any side state (ASID, generation counter, etc. in
/// future). Kept field-compatible with the RISC-V port so top-level code
/// can use the same `Option<PTEFlags>` shapes.
pub struct PageTable {
    pub root: usize,
}

impl PageTable {
    pub const fn new() -> Self {
        Self { root: 0 }
    }

    /// Wrap an existing page-directory root (used by `kernelpagetable::init`
    /// once we have an early-allocated PGD).
    pub fn from_root(root: usize) -> Self {
        debug_assert!(root != 0, "PageTable root cannot be zero");
        Self { root }
    }

    /// Populate an empty `PageTable::new()` with a freshly allocated root.
    pub fn create(&mut self) {
        unimplemented!("loongarch: PageTable::create (Phase 3)");
    }

    /// Value to program into CSR.PGDL for this address space. Phase 3 will
    /// return `root & ~0xfff` (LoongArch keeps no mode/asid bits in PGDL).
    pub fn get_pgd(&self) -> usize {
        unimplemented!("loongarch: PageTable::get_pgd (Phase 3)");
    }

    pub fn is_mapped(&self, _uaddr: usize) -> bool {
        unimplemented!("loongarch: PageTable::is_mapped (Phase 3)");
    }

    pub fn mapped_flag(&self, _uaddr: usize) -> Option<PTEFlags> {
        unimplemented!("loongarch: PageTable::mapped_flag (Phase 3)");
    }

    pub fn mapped_perm(&self, _uaddr: usize) -> Option<MapPerm> {
        unimplemented!("loongarch: PageTable::mapped_perm (Phase 3)");
    }

    /// Kernel-space identity mapping helper, used by
    /// `kernelpagetable::map_kernel_addr`.
    pub fn mmap_kernel(&mut self, _kaddr: usize, _paddr: usize, _perm: MapPerm) {
        unimplemented!("loongarch: PageTable::mmap_kernel (Phase 3)");
    }

    /// Software A/D emulation: return true if the page was marked for the
    /// first time. On LoongArch the hardware sets D on first store (via PME
    /// exception + software `tlbfill`), and A on first load. We'll still
    /// keep these shims because a lot of generic mm code calls them after
    /// fault fix-up.
    pub fn mark_page_accessed(&mut self, _uaddr: usize) -> bool {
        unimplemented!("loongarch: PageTable::mark_page_accessed (Phase 3)");
    }

    pub fn mark_page_accessed_and_dirty(&mut self, _uaddr: usize) -> bool {
        unimplemented!("loongarch: PageTable::mark_page_accessed_and_dirty (Phase 3)");
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        // Phase 3 will recursively free intermediate directories. For now
        // the stub never allocates a root, so dropping is a no-op.
        self.root = 0;
    }
}

unsafe impl Send for PageTable {}
unsafe impl Sync for PageTable {}

impl PageTableTrait for PageTable {
    fn mmap(&mut self, _uaddr: usize, _kaddr: usize, _perm: MapPerm) {
        unimplemented!("loongarch: PageTable::mmap (Phase 3)");
    }

    fn mmap_paddr(&mut self, _kaddr: usize, _paddr: usize, _perm: MapPerm) {
        unimplemented!("loongarch: PageTable::mmap_paddr (Phase 3)");
    }

    fn mmap_replace(&mut self, _uaddr: usize, _kaddr: usize, _perm: MapPerm) {
        unimplemented!("loongarch: PageTable::mmap_replace (Phase 3)");
    }

    fn mmap_replace_kaddr(&mut self, _uaddr: usize, _kaddr: usize) {
        unimplemented!("loongarch: PageTable::mmap_replace_kaddr (Phase 3)");
    }

    fn mmap_replace_perm(&mut self, _uaddr: usize, _perm: MapPerm) {
        unimplemented!("loongarch: PageTable::mmap_replace_perm (Phase 3)");
    }

    fn munmap(&mut self, _uaddr: usize) {
        unimplemented!("loongarch: PageTable::munmap (Phase 3)");
    }

    fn munmap_with_check(&mut self, _uaddr: usize, _expected_kaddr: usize) -> bool {
        unimplemented!("loongarch: PageTable::munmap_with_check (Phase 3)");
    }

    fn take_access_dirty_bit(&mut self, _uaddr: usize) -> Option<(bool, bool)> {
        unimplemented!("loongarch: PageTable::take_access_dirty_bit (Phase 3)");
    }
}
