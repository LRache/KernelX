use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::arch;
use crate::kernel::mm::page;

pub struct VirtIOHal;

unsafe impl Hal for VirtIOHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let kaddr = page::alloc_contiguous(pages);
        let ptr = NonNull::new(kaddr as *mut u8).expect("Failed to allocate DMA memory");
        (arch::kaddr_to_paddr(kaddr), ptr)
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let kaddr = vaddr.as_ptr() as usize;
        page::free_contiguous(kaddr, pages);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        // The physical address here is in the PCI memory-mapped region
        // (e.g. a BAR we allocated into the bridge's 32-bit window). It
        // MUST NOT be accessed cached — the device observes reads and
        // writes through the same transaction stream as the CPU's stores.
        // `arch::mmio_phys_to_kaddr` returns:
        //   - RISC-V: a freshly mapped kernel VA with RW+MMIO semantics.
        //   - LoongArch: the DMW0 (uncached, strongly-ordered) mirror of
        //     the PA.
        let kaddr = arch::mmio_phys_to_kaddr(paddr, size);
        NonNull::new(kaddr as *mut u8).expect("Failed to convert MMIO physical address to virtual address")
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        arch::kaddr_to_paddr(buffer.as_ptr() as *mut u8 as usize)
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Unsharing logic if needed
    }
}
