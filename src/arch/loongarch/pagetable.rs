//! LoongArch64 three-level page table (9-9-9-12, 48-bit VA, 48-bit PA).
//!
//! Shape-for-shape port of `src/arch/riscv/pagetable/` with the encoding
//! swapped out for LoongArch LA64 rules. The walker (`find_pte_or_create`,
//! `free_pagetable`) is mechanically identical to RISC-V — LoongArch happens
//! to share the same 9-9-9-12 geometry at 4 KiB pages.
//!
//! What's different from RISC-V:
//!   - PTE entries are **64-bit** and flags live across the whole word
//!     (NR/NX/RPLV are in bits 61-63). We therefore keep `PTEFlags` as u64
//!     directly — no u16 mirror of the high bits — so write-back is a plain
//!     `bits() | (ppn << 12)` and not three XORs.
//!   - No hardware A bit. We substitute a software P (present) bit in the
//!     bits 7-11 reserved range. Hardware does set D on first store; we read
//!     and clear it the same way RISC-V does.
//!   - PPN occupies bits [47:12] (36-bit PPN given PALEN=48) instead of
//!     RISC-V's [53:10].
//!   - Priv control is a 2-bit PLV field (0=kernel, 3=user), not a single
//!     U bit. `MapPerm::U` -> PLV3; absence -> PLV0.
//!   - Read/Exec are **negative** (NR / NX) — so `MapPerm::R` means clear NR.
//!
//! What's the same:
//!   - VPN slicing: `[ (v>>30)&0x1ff, (v>>21)&0x1ff, (v>>12)&0x1ff ]`.
//!   - Walker alloc-on-demand, Drop recursively free.
//!   - 4 KiB base page, 512 entries per directory.
//!
//! CSR-side bring-up (PGDL/PGDH/PWCL/PWCH/STLBPS) is Phase 4/5 — Phase 3
//! only builds the data structures.

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
    /// Real LoongArch PTE bit layout (PALEN=48, 4 KiB pages, RPLV=0 everywhere
    /// we care about). Bit positions match hardware directly; there is no
    /// separate "mirror" representation. Vol.1 §8.5.1.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PTEFlags: u64 {
        /// Valid — set on any PTE that participates in translation. Software
        /// walker uses this to detect "table not yet allocated" at intermediate
        /// levels too; see `find_pte_or_create`.
        const V    = 1 << 0;
        /// Dirty — hardware sets this on the first store through the mapping.
        const D    = 1 << 1;
        /// PLV = 0 (kernel-only access).
        const PLV0 = 0 << 2;
        /// PLV = 3 (user-accessible).
        const PLV3 = 3 << 2;
        /// MAT = 0: strongly uncached. Use only for MMIO mapped through
        /// page tables (Phase 3 currently relies on DMW0 for all MMIO).
        const SUC  = 0 << 4;
        /// MAT = 1: cached coherent. This is what every normal RAM page wants.
        const CC   = 1 << 4;
        /// MAT = 2: weakly uncached. Rarely used.
        const WUC  = 2 << 4;
        /// Global — translation shared across ASIDs.
        const G    = 1 << 6;
        /// Software-used "present" bit. LoongArch has no hardware Accessed
        /// bit; we repurpose bit 7 (reserved in the hardware PTE) to stand in
        /// for RISC-V's PTEFlags::A. Cleared by `take_access_dirty_bit`.
        const P    = 1 << 7;
        /// Software "writable" tracking. LoongArch's hardware writable bit is
        /// bit 8 per some references; we keep the same position and treat it
        /// as the single source of truth for "will a store succeed".
        const W    = 1 << 8;
        /// Not readable — when set, a load through this PTE raises PIL.
        const NR   = 1 << 61;
        /// Not executable — when set, an ifetch through this PTE raises PIF.
        const NX   = 1 << 62;
        /// Restrict PLV — when set, only the exact PLV in the PTE may access.
        const RPLV = 1 << 63;
    }
}

impl From<MapPerm> for PTEFlags {
    fn from(perm: MapPerm) -> Self {
        // Base: valid + cached + present. We add R/W/X "by subtraction" via
        // NR/NX for non-read/non-exec. MapPerm is always a positive-permission
        // set, so start from "no access" and OR in the allowed directions.
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

// -------- Address helpers (mirrors `src/arch/riscv/pagetable/pte.rs`) --------

/// A thin wrapper that carries an address plus an interpretation. We cross
/// freely between kaddr (DMW1) and paddr here — every `alloc_zero` we call
/// returns a kaddr, and every PPN we store in a PTE is a paddr-shifted.
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

    pub const fn vaddr(self) -> usize {
        self.0
    }
    pub const fn kaddr(self) -> usize {
        self.0
    }
    pub fn paddr(self) -> usize {
        kaddr_to_paddr(self.0)
    }

    pub const fn pgoff(self) -> usize {
        self.0 & PGMASK
    }

    /// 48-bit VA split into three 9-bit indices (top to bottom). Same geometry
    /// as RISC-V Sv39, which is why we can mechanically port the walker.
    pub const fn vpn(self) -> [usize; PAGE_TABLE_LEVELS] {
        [
            (self.0 >> 30) & 0x1ff,
            (self.0 >> 21) & 0x1ff,
            (self.0 >> 12) & 0x1ff,
        ]
    }

    /// Turn this (kernel-view) address into a PPN suitable for embedding in
    /// a PTE. Paddr is taken first so that DMW1 high bits are not left in.
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

// ------------------------------ PTE ------------------------------

/// Single page-table entry. Carries its own write-back pointer so the walker
/// can return a PTE by value and the caller still mutates the in-memory slot.
///
/// Read-modify-write flow (see any `PageTableTrait` impl below):
///     let mut pte = self.find_pte_or_create(vaddr);
///     pte.set_flags(new_flags);
///     pte.set_ppn(new_ppn);
///     pte.write_back().expect("...");
#[derive(Debug, Clone, Copy)]
pub struct PTE {
    bits: u64,
    ptr: Option<NonNull<u64>>,
}

/// PPN bit mask inside the hardware PTE, pre-shifted: bits [47:12], i.e.
/// 36 contiguous bits at position 12.
const PPN_SHIFT: u64 = PGBITS as u64;
const PPN_BITS: u64 = 36; // PALEN (48) - PGBITS (12)
const PPN_MASK_IN_PTE: u64 = ((1u64 << PPN_BITS) - 1) << PPN_SHIFT;
/// Combined mask of all bits PTEFlags claims. Used to strip flags before
/// writing a fresh PPN.
const FLAG_BITS_MASK: u64 = PTEFlags::all().bits();

impl PTE {
    /// Load from a 64-bit slot. Panics on null pointer (walker never produces
    /// one; only misuse would).
    pub fn from_ptr(ptr: NonNull<u64>) -> Self {
        let bits = unsafe { ptr.as_ptr().read_volatile() };
        Self {
            bits,
            ptr: Some(ptr),
        }
    }

    pub fn from_raw_ptr(ptr: *mut u64) -> Self {
        Self::from_ptr(NonNull::new(ptr).expect("PTE pointer must not be null"))
    }

    pub const fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits & FLAG_BITS_MASK)
    }

    /// Full raw 64-bit PTE image as it will be written back to the page
    /// table entry slot. Useful for trace / debug prints when verifying
    /// hardware expectations against what we've constructed.
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

    /// Descend into the next-level table this PTE points to. Only sensible
    /// on intermediate levels — walker asserts that a child table has been
    /// installed.
    ///
    /// Note the assertion checks `ppn().value() != 0`, NOT `is_valid()`.
    /// Intermediate entries on LoongArch don't use the V bit because `lddir`
    /// treats the whole 64-bit slot as the child-table base PA (AND'd with
    /// the PALEN mask). A non-zero PPN is our "installed" signal.
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

// ------------------------------ PTETable ------------------------------

/// View onto a 4 KiB page treated as an array of 512 `u64` PTE slots. The
/// base pointer is a kernel address (DMW1) — reading and writing through
/// it goes via DMW with cache-coherent attributes (MAT=CC).
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

// ------------------------------ PageTable ------------------------------

pub struct PageTable {
    /// Kernel-view pointer to the root directory (allocated via
    /// `mm::page::alloc_zero`, which hands out a kaddr in DMW1). Stored as
    /// `usize` to keep the struct field-compatible with the RISC-V port.
    ///
    /// A value of `0` means the page table is not yet backed (post-`new`,
    /// pre-`create`). `Drop` checks for that so a never-used `PageTable` is
    /// cheap.
    pub root: usize,
}

impl PageTable {
    pub const fn new() -> Self {
        Self { root: 0 }
    }

    /// Wrap an existing root (reserved for a hypothetical kernel-pagetable
    /// use case). Currently unused on LoongArch because DMW obviates the need
    /// for a kernel page table.
    #[allow(dead_code)]
    pub fn from_root(root: usize) -> Self {
        debug_assert!(root != 0, "PageTable root cannot be zero");
        Self { root }
    }

    /// Allocate a zeroed page and install it as the root. Called by
    /// `AddrSpace::new`.
    pub fn create(&mut self) {
        debug_assert!(self.root == 0, "PageTable::create called twice");
        self.root = mm::page::alloc_zero();
    }

    /// Value to program into CSR.PGDL / CSR.PGDH at context switch (Phase 5).
    /// The CSR wants a physical address of the root directory, page-aligned.
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

    /// Software accessed emulation. LoongArch has no hardware A bit; we use
    /// `PTEFlags::P` as a stand-in and the fault handler (Phase 4) will set
    /// it on first touch.
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

    /// Used by kernel-range mappings; currently unreachable on LoongArch
    /// because `Arch::map_kernel_addr` is a no-op (DMW1 covers all of RAM).
    /// Kept for API parity and future MMIO-via-pagetable cases.
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
                // Leaf: V bit decides presence, matching what `ldpte` will
                // enforce.
                return if pte.is_valid() { Some(pte) } else { None };
            }

            // Intermediate: "installed" means the slot holds a non-zero
            // child-table PA. See `find_pte_or_create` for why we don't
            // set V on intermediate entries.
            if pte.ppn().value() == 0 {
                return None;
            }

            ptetable = pte.next_level();
        }

        unreachable!("find_pte walker should always return within the loop")
    }

    /// Walk the three-level table, allocating intermediate directories on
    /// demand. Returns the leaf PTE's loaded-and-pointable view; the caller
    /// mutates via `set_flags`/`set_ppn` and `write_back`.
    fn find_pte_or_create(&mut self, vaddr: usize) -> PTE {
        debug_assert!(self.root != 0);
        let vpns = Addr::from_vaddr(vaddr).vpn();

        let mut ptetable = PTETable::new(self.root as *mut u64);

        for level in 0..PAGE_TABLE_LEVELS {
            let mut pte = ptetable.get(vpns[level]);

            if level == LEAF_LEVEL {
                return pte;
            }

            // Intermediate-level "not installed" is indicated by a zero PPN
            // in the slot, NOT by a cleared V bit — see the long comment
            // below for why V must stay off on intermediate entries.
            if pte.ppn().value() == 0 {
                // Allocate the next-level directory and write a pure PA
                // pointer into this entry. On LoongArch, `lddir` uses the
                // WHOLE 64-bit entry as the child-table base (AND'd with
                // the PALEN mask) — it does NOT shift out a PPN field or
                // check any validity bit on intermediate levels. So we
                // must store the child-table PA with the low 12 bits clear
                // and nothing else set. In particular, setting V (bit 0)
                // here would misalign the subsequent `lddir` load by 1
                // byte; setting any flag bit other than HUGE / LEVEL would
                // likewise corrupt the address. QEMU's `helper_lddir`
                // confirms this: it just AND's with the PALEN mask before
                // the indexed read.
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

    /// Recursively free every directory allocated by the walker (interior
    /// nodes and the root), then the root slot. Called from `Drop`.
    fn free_pagetable(&mut self, ptetable: &PTETable, level: usize) {
        if level != LEAF_LEVEL {
            for i in 0..ENTRIES_PER_TABLE {
                let pte = ptetable.get(i);
                // Intermediate slots use non-zero PPN as the "installed"
                // marker; leaf slots use V. We're only recursing through
                // intermediate levels here (guarded by `level != LEAF_LEVEL`).
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
        // Stub / "never created" case: nothing to free. This protects against
        // `PageTable::new()` being dropped without any `create`, which RISC-V
        // does not guard against but we cheaply can.
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

// ------------------------ PageTableTrait impls ------------------------
//
// Shape copied from `src/arch/riscv/pagetable/pagetable.rs:200-295`. The only
// semantic difference is the "pre-populate access/dirty" trick: RISC-V sets
// A|D up front so the CPU doesn't trap on first access; on LoongArch we set
// P|D instead (P is our software accessed stand-in).

impl PageTableTrait for PageTable {
    fn mmap(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let mut flags: PTEFlags = perm.into();
        flags |= PTEFlags::P | PTEFlags::D;

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

    fn mmap_paddr(&mut self, kaddr: usize, paddr: usize, perm: MapPerm) {
        let flags: PTEFlags = perm.into();

        let mut pte = self.find_pte_or_create(kaddr);
        pte.set_flags(flags);
        pte.set_ppn(PPN::from_paddr(paddr));
        pte.write_back()
            .expect("Failed to write back PTE on mmap_paddr");
    }

    fn mmap_replace(&mut self, uaddr: usize, kaddr: usize, perm: MapPerm) {
        let flags: PTEFlags = perm.into();

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back()
            .expect("Failed to write back PTE on mmap_replace");
    }

    fn mmap_replace_kaddr(&mut self, uaddr: usize, kaddr: usize) {
        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_ppn(Addr::from_kaddr(kaddr).ppn());
        pte.write_back()
            .expect("Failed to write back PTE on mmap_replace_kaddr");
    }

    fn mmap_replace_perm(&mut self, uaddr: usize, perm: MapPerm) {
        let flags: PTEFlags = perm.into();

        let mut pte = self.find_pte_or_create(uaddr);
        pte.set_flags(flags);
        pte.write_back()
            .expect("Failed to write back PTE on mmap_replace_perm");
    }

    fn munmap(&mut self, uaddr: usize) {
        let mut pte = self.find_pte(uaddr).expect("PTE not found for munmap");
        pte.set_flags(PTEFlags::empty());
        pte.write_back()
            .expect("Failed to write back PTE on munmap");
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
