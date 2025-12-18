use crate::{arch, kernel::mm::{MapPerm, page}};

pub enum DeviceType {
    Block,
    Char,
    Rtc,
}

#[derive(Debug)]
pub struct Device<'a> {
    mmio_base: usize,
    mmio_size: usize,
    name: &'a str,
    compatible: &'a str,
    interrupt_number: Option<u32>,
}

impl<'a> Device<'a> {
    pub fn new(mmio_base: usize, mmio_size: usize, name: &'a str, compatible: &'a str, interrupt_number: Option<u32>) -> Device<'a> {
        Device {
            mmio_base,
            mmio_size,
            name,
            compatible,
            interrupt_number,
        }
    }

    pub fn mmio_base(&self) -> usize {
        self.mmio_base
    }

    pub fn mmio_size(&self) -> usize {
        self.mmio_size
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn compatible(&self) -> &'a str {
        self.compatible
    }

    pub fn interrupt_number(&self) -> Option<u32> {
        self.interrupt_number
    }

    pub fn alloc_mmio_pages(&self) -> usize {
        let kbase = page::alloc_contiguous(arch::page_count(self.mmio_size()));
        arch::map_kernel_addr(kbase, self.mmio_base(), self.mmio_size(), MapPerm::RW);
        kbase
    }
}
