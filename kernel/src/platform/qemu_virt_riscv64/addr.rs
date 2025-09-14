use super::config::KERNEL_VADDR_OFFSET;

pub fn kaddr_to_paddr(kaddr: usize) -> usize {
    kaddr - KERNEL_VADDR_OFFSET
}

pub fn paddr_to_kaddr(paddr: usize) -> usize {
    paddr + KERNEL_VADDR_OFFSET
}
