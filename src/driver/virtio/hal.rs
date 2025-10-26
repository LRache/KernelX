
use virtio_drivers::{Hal, BufferDirection, PhysAddr};
use core::ptr::NonNull;

use crate::kernel::mm::page;
use crate::arch;

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

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as * mut u8).expect("Failed to convert MMIO physical address to virtual address")
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        return arch::kaddr_to_paddr(buffer.as_ptr() as *mut u8 as usize);
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Unsharing logic if needed
    }
}