use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::arch;
use crate::kernel::mm::page;

pub struct VirtIOHal;

fn alloc_zeroed_dma_pages(pages: usize) -> usize {
    let kaddr = page::alloc_contiguous_zero(pages);
    // SAFETY: `kaddr` is an exclusive allocation of `pages` contiguous,
    // writable pages, and `size` is exactly the allocation size.
    kaddr
}

unsafe impl Hal for VirtIOHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let kaddr = alloc_zeroed_dma_pages(pages);
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

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let kaddr = buffer.as_ptr() as *mut u8 as usize;
        let len = buffer.len();

        if let Some(paddr) = arch::dma_direct_paddr(kaddr, len) {
            return paddr;
        }

        let bounce_kaddr = page::alloc_contiguous(arch::page_count(len));
        if matches!(direction, BufferDirection::DriverToDevice | BufferDirection::Both) {
            // SAFETY: `buffer` is valid for `len` bytes by the Hal contract,
            // and `bounce_kaddr` owns at least `len` writable bytes.
            unsafe { core::ptr::copy_nonoverlapping(kaddr as *const u8, bounce_kaddr as *mut u8, len) };
        }
        arch::kaddr_to_paddr(bounce_kaddr)
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        let kaddr = buffer.as_ptr() as *mut u8 as usize;
        let len = buffer.len();

        if arch::dma_direct_paddr(kaddr, len) == Some(paddr) {
            return;
        }

        let bounce_kaddr = arch::paddr_to_kaddr(paddr);
        if matches!(direction, BufferDirection::DeviceToDriver | BufferDirection::Both) {
            // SAFETY: `paddr` is the contiguous bounce allocation returned by
            // the matching `share`, and the device has finished using it.
            unsafe { core::ptr::copy_nonoverlapping(bounce_kaddr as *const u8, kaddr as *mut u8, len) };
        }
        page::free_contiguous(bounce_kaddr, arch::page_count(len));
    }
}
