use bitflags::bitflags;
use core::ptr::NonNull;
use core::fmt;

use crate::arch::riscv::{PGBITS, PGMASK};
use crate::kernel::mm;
use crate::{platform, println};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr(usize);

impl Addr {
    pub fn new<T>(addr: *const T) -> Self {
        Addr(addr as usize)
    }

    pub fn from_paddr(paddr: usize) -> Self {
        // Addr(paddr + platform::config::KERNEL_VADDR_OFFSET)
        let addr = paddr.checked_add(platform::config::KERNEL_VADDR_OFFSET);
        match addr {
            Some(addr) => Addr(addr),
            None => {
                println!("Address overflow when converting paddr to Addr: 0x{:x}", paddr);
                panic!("Address overflow");
            }
        }
    }

    pub const fn from_kaddr(kaddr: usize) -> Self {
        Addr(kaddr)
    }

    pub const fn from_vaddr(vaddr: usize) -> Self {
        Addr(vaddr)
    }

    pub fn paddr(self) -> usize {
        self.0 - platform::config::KERNEL_VADDR_OFFSET
    }

    pub const fn kaddr(self) -> usize {
        self.0
    }

    pub const fn vaddr(self) -> usize {
        self.0
    }

    pub const fn pgoff(self) -> usize {
        self.0 & PGMASK
    }

    pub const fn vpn(self) -> [usize; 3] {
        [
            (self.0 >> 30) & 0x1ff,
            (self.0 >> 21) & 0x1ff,
            (self.0 >> 12) & 0x1ff,
        ]
    }

    pub fn ppn(self) -> PPN {
        assert!(self.paddr() < 0x88000000, "PPN out of range: 0x{:x}", self.paddr());
        PPN::new(self.paddr() >> PGBITS)
    }

    pub const fn ptr(self) -> *mut usize {
        self.0 as *mut usize
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

impl From<Addr> for *mut usize {
    fn from(addr: Addr) -> Self {
        addr.ptr()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PPN {
    ppn: usize
}

impl PPN {
    pub fn new(ppn: usize) -> Self {
        // assert!((ppn << PGBITS) < 0x88000000, "PPN out of range: 0x{:x}", ppn);
        PPN { ppn }
    }

    pub const fn value(self) -> usize {
        self.ppn
    }

    pub fn to_paddr(self) -> usize {
        assert!((self.ppn << PGBITS) < 0x88000000, "PPN out of range: 0x{:x}", self.ppn);
        self.ppn << PGBITS
    }

    pub const fn from_paddr(paddr: usize) -> Self {
        PPN { ppn: paddr >> PGBITS }
    }

    pub fn to_addr(self) -> Addr {
        Addr::from_paddr(self.to_paddr())
    }
}

impl fmt::Display for PPN {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PPN(0x{:x})", self.ppn)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PTE {
    pte: usize,
    ptr: Option<NonNull<usize>>,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PTEFlags: u16 {
        const V = 1 << 0; // Valid bit
        const R = 1 << 1; // Readable
        const W = 1 << 2; // Writable
        const X = 1 << 3; // Executable
        const U = 1 << 4; // User-accessible
        const G = 1 << 5; // Global page
        const A = 1 << 6; // Accessed bit
        const D = 1 << 7; // Dirty bit
        const N = 1 << 8; // Don't Clone
    }
}

impl PTE {
    pub fn from_ptr(ptr: NonNull<usize>) -> Self {
        let pte = unsafe { ptr.read() };
        Self {
            pte,
            ptr: Some(ptr),
        }
    }

    pub fn from_raw_ptr(ptr: *mut usize) -> Self {
        Self::from_ptr(NonNull::new(ptr).expect("PTE pointer cannot be null"))
    }

    pub const fn pte(&self) -> usize {
        self.pte
    }

    pub const fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate((self.pte & 0x1ff) as u16)
    }

    pub fn set_flags(&mut self, flags: PTEFlags) {
        self.pte = (self.pte & !0x1ff) | (flags.bits() as usize);
    }

    pub fn ppn(self) -> PPN {
        PPN::new((self.pte >> 10) & ((1 << 44) - 1))
    }

    pub fn set_ppn(&mut self, ppn: PPN) -> &mut Self {
        self.pte = (self.pte & !(((1 << 44) - 1) << 10)) | (ppn.value() << 10);
        self
    }

    pub fn next_level(&self) -> PTETable {
        PTETable::new(self.page())
    }

    pub fn page(&self) -> *mut usize {
        self.ppn().to_addr().ptr()
    }

    pub fn write_back(&self) -> Result<(), ()> {
        match self.ptr {
            Some(ptr) => {
                unsafe { ptr.write(self.pte) };
                Ok(())
            }
            None => Err(()),
        }
    }

    pub const fn is_valid(self) -> bool {
        self.flags().contains(PTEFlags::V)
    }

    pub const fn dont_clone(self) -> bool {
        self.flags().contains(PTEFlags::N)
    }
}

impl fmt::Display for PTE {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PTE(0x{:016x}, {})", self.pte, self.ppn())
    }
}

pub struct PTETable {
    base: *mut usize
}

impl PTETable {
    pub fn new(base: *mut usize) -> Self {
        PTETable { base }
    }

    pub fn get(&self, index: usize) -> PTE {
        PTE::from_raw_ptr(unsafe { self.base.add(index) })
    }

    pub fn set(&mut self, index: usize, pte: PTE) {
        unsafe { self.base.add(index).write(pte.pte) };
    }

    pub fn free(&self) {
        mm::page::free(self.base as usize);
    }
}

impl From<*mut usize> for PTETable {
    fn from(base: *mut usize) -> Self {
        PTETable::new(base)
    }
}

impl Into<PTETable> for PTE {
    fn into(self) -> PTETable {
        PTETable::new(self.ppn().to_addr().ptr() as *mut usize)
    }
}
